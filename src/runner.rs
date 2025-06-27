use check::{map_command_exit_code_to_check_result, CheckResult};
use chrono::prelude::*;
use chrono::Local;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use kube::api::DeleteParams;
use kube::api::ListParams;
use kube::api::PostParams;
use kube::api::PropagationPolicy;
use kube::runtime::conditions;
use kube::runtime::wait::await_condition;
use kube::{Api, Client};
use std::{collections::BinaryHeap, time::Duration};

use tokio::{
    sync::mpsc,
    time::{sleep_until, Instant},
};

use crate::check::CheckResultStatus;
use crate::check::{self, RunnableCheck, ScheduledCheck};

/**
 * This function continuously schedule checks based on the interval
 */
pub async fn scheduler_loop(
    checks: Vec<RunnableCheck>,
    result_tx: mpsc::Sender<CheckResult>,
    namespace: String,
) {
    // Create the queue of checks
    let mut queue = BinaryHeap::new();

    let now = Instant::now();

    // Add all checks in the queue. The queue will be automatically ordered by next run
    for check in checks {
        queue.push(ScheduledCheck {
            next_run: now + Duration::from_secs(check.interval),
            check,
        });
    }

    // Continuosly loop
    loop {
        if let Some(mut scheduled) = queue.pop() {
            let now = Instant::now();
            // If the next check must not yet be run just sleep
            if scheduled.next_run > now {
                sleep_until(scheduled.next_run).await;
            }

            let check_interval = Duration::from_secs(scheduled.check.interval);

            // Run the check asynchronously
            let tx = result_tx.clone();
            tokio::spawn(run_check(scheduled.check.clone(), tx, namespace.clone()));

            // Schedule the next run
            scheduled.next_run += check_interval;
            queue.push(scheduled);
        } else {
            // If no checks are scheduled, just sleep briefly to prevent tight loop
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

async fn send_check_result(
    result_tx: mpsc::Sender<CheckResult>,
    check_name: String,
    check_output: String,
    status: CheckResultStatus,
) {
    let timestamp = Local::now().to_rfc3339();
    let result = CheckResult {
        check_name,
        output: check_output,
        status,
        timestamp,
    };
    result_tx.send(result).await.unwrap();
}

/**
 * This function runs a check, parses the result to a check result and returns it to the main thread
 */
pub async fn run_check(
    check: RunnableCheck,
    result_tx: mpsc::Sender<CheckResult>,
    namespace: String,
) {
    let client = Client::try_default().await.unwrap();
    let job_name = format!(
        "bash-check-{}-{}",
        check.check_name,
        Utc::now().format("%Y%m%d%H%M%S")
    );
    let job = build_bash_job(&job_name, &check.script);

    let jobs: Api<Job> = Api::namespaced(client.clone(), &namespace);

    // Create the job
    println!("Creating job...");
    if let Err(e) = jobs.create(&PostParams::default(), &job).await {
        send_check_result(
            result_tx,
            check.check_name.clone(),
            format!("Error creating Kubernetes job for check. {:?}", e),
            CheckResultStatus::CheckError,
        )
        .await;
        return;
    }

    // Wait for the job to complete
    println!("Waiting for job to complete...");
    let _ = await_condition(jobs.clone(), &job_name, conditions::is_job_completed()).await;

    // Get the Pod created by the Job
    let pods: Api<Pod> = Api::namespaced(client.clone(), &namespace);
    let lp = ListParams::default().labels(&format!("job-name={}", job_name));
    let pod_list = pods.list(&lp).await;

    if let Err(pod_list_err) = pod_list {
        send_check_result(
            result_tx.clone(),
            check.check_name.clone(),
            format!("Error retrieving check execution pod: {:?}", pod_list_err),
            CheckResultStatus::CheckError,
        )
        .await;
        return;
    }

    let pod = pod_list
        .expect("Error should have been catched")
        .items
        .into_iter()
        .next()
        .expect("No pod found for job");

    let pod_name = pod.metadata.name.clone().unwrap();

    // Get stdout logs
    let logs = pods.logs(&pod_name, &Default::default()).await;
    if let Err(logs_err) = logs {
        send_check_result(
            result_tx.clone(),
            check.check_name.clone(),
            format!("Error retrieving check execution logs: {:?}", logs_err),
            CheckResultStatus::CheckError,
        )
        .await;
        return;
    }
    let logs = logs.map_err(|_e| {}).unwrap();

    // Extract the container exit code
    if let Some(statuses) = &pod.status.and_then(|s| s.container_statuses) {
        if let Some(exit_code) = statuses[0]
            .state
            .as_ref()
            .and_then(|s| s.terminated.as_ref())
            .map(|t| t.exit_code)
        {
            send_check_result(
                result_tx,
                check.check_name,
                logs,
                map_command_exit_code_to_check_result(Some(exit_code)),
            )
            .await;
            return;
        }
    }

    send_check_result(
        result_tx,
        check.check_name,
        "Error retrieving check exit code from pod".to_string(),
        CheckResultStatus::CheckError,
    )
    .await;
    jobs.delete(
        &job_name,
        &DeleteParams {
            propagation_policy: Some(PropagationPolicy::Foreground), // deletes Pods too
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

fn build_bash_job(job_name: &str, check_script: &str) -> Job {
    // Escape newlines and quotes to run inline in bash -c
    //let escaped_script = check_script.replace('"', "\\\"").replace('\n', "; ");
    let escaped_script = check_script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");

    let escaped_script = format!("set -euo pipefail; {}", escaped_script);

    // Build the Job object
    Job {
        metadata: kube::api::ObjectMeta {
            name: Some(job_name.to_owned()),
            ..Default::default()
        },
        spec: Some(k8s_openapi::api::batch::v1::JobSpec {
            ttl_seconds_after_finished: Some(60),
            template: k8s_openapi::api::core::v1::PodTemplateSpec {
                spec: Some(k8s_openapi::api::core::v1::PodSpec {
                    containers: vec![k8s_openapi::api::core::v1::Container {
                        name: "bash-script".to_string(),
                        image: Some("bash:latest".into()),
                        command: Some(vec!["bash".into(), "-c".into(), escaped_script]),
                        ..Default::default()
                    }],
                    restart_policy: Some("Never".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            backoff_limit: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    }
}
