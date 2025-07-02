use check::{map_command_exit_code_to_check_result, CheckResult};
use chrono::prelude::*;
use chrono::Local;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use kube::api::DeleteParams;
use kube::api::ListParams;
use kube::api::PostParams;
use kube::api::PropagationPolicy;
use kube::runtime::wait::await_condition;
use kube::{Api, Client};
use log::debug;
use log::error;
use std::{collections::BinaryHeap, time::Duration};

use tokio::{
    sync::mpsc,
    time::{sleep_until, Instant},
};

use crate::check::{self, RunnableCheck, ScheduledCheck};
use crate::job::build_bash_job;
use crate::job::build_python_job;
use crate::job::is_job_finished;

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

/**
 * This function runs a check, parses the result to a check result and returns it to the main thread
 */
pub async fn run_check(
    check: RunnableCheck,
    result_tx: mpsc::Sender<CheckResult>,
    namespace: String,
) {
    // Run the check as Kubernetes job
    let mut check_result = match run_check_as_kube_job(check, namespace).await {
        Ok(res) | Err(res) => res,
    };

    // Inject the timestamp of when we completed the check execution
    let timestamp = Local::now().to_rfc3339();
    check_result.set_check_result_timestamp(timestamp);

    // Send the check result
    if let Err(e) = result_tx.send(check_result).await {
        error!("Error sending check result: {:?}", e)
    }
}

/**
 * This function runs a check as a Kubernetes job
 */
async fn run_check_as_kube_job(
    check: RunnableCheck,
    namespace: String,
) -> Result<CheckResult, CheckResult> {
    let check_name = &check.check_name;
    // Create the job name
    let job_name = format!(
        "{}-check-{}-{}",
        check.language,
        check_name,
        Utc::now().format("%Y%m%d%H%M%S")
    );
    // Create the client to interact with the Kube API
    let client = Client::try_default().await.map_err(|e| {
        CheckResult::map_to_check_error(
            check_name,
            format!("Error when creating the Kubernetes client: {:?}", e),
        )
    })?;

    // Build the job
    let job = match check.language {
        check::ScriptLanguage::Python => build_python_job(
            &job_name,
            &check.script,
            check.secrets_refs,
            check.python_requirements,
        ),
        check::ScriptLanguage::Bash => build_bash_job(&job_name, &check.script, check.secrets_refs),
    };

    // Get the job API
    let jobs: Api<Job> = Api::namespaced(client.clone(), &namespace);

    debug!("Creating job for check {}", check_name);

    // Create the job
    jobs.create(&PostParams::default(), &job)
        .await
        .map_err(|e| {
            CheckResult::map_to_check_error(
                check_name,
                format!("Error when creating the Kubernetes job: {:?}", e),
            )
        })?;

    // Wait for the job to complete
    debug!(
        "Waiting for job {} for check {} to complete...",
        job_name, check_name
    );
    let _ = await_condition(jobs.clone(), &job_name, is_job_finished()).await;

    // Get the Pod created by the Job
    let pods: Api<Pod> = Api::namespaced(client.clone(), &namespace);
    let lp = ListParams::default().labels(&format!("job-name={}", job_name));
    let pod_list = pods.list(&lp).await.map_err(|e| {
        CheckResult::map_to_check_error(
            check_name,
            format!("Error when retrieving the pods list: {:?}", e),
        )
    })?;

    let pod = pod_list.items.into_iter().next().ok_or_else(|| {
        CheckResult::map_to_check_error(check_name, format!("Cannot find pod for job {}", job_name))
    })?;

    let pod_name = pod.metadata.name.clone().ok_or_else(|| {
        CheckResult::map_to_check_error(
            check_name,
            format!("Error getting pod name from pod {:?}", pod),
        )
    })?;

    // Get the pod logs
    let logs = pods
        .logs(&pod_name, &Default::default())
        .await
        .map_err(|e| {
            CheckResult::map_to_check_error(check_name, format!("Cannot get pod logs: {:?}", e))
        })?;

    // Get the exit code of the pod
    let exit_code = &pod
        .status
        .and_then(|s| s.container_statuses)
        .and_then(|s| s[0].state.clone())
        .and_then(|s| s.terminated.as_ref().map(|t| t.exit_code));

    // Delete the job and corresponding pod
    if let Err(e) = jobs
        .delete(
            &job_name,
            &DeleteParams {
                propagation_policy: Some(PropagationPolicy::Foreground), // deletes Pods too
                ..Default::default()
            },
        )
        .await
    {
        error!(
            "Error when deleting job after completion: {:?} - {:?}",
            job_name, e
        )
    }

    Ok(CheckResult {
        check_name: check_name.to_string(),
        output: logs,
        status: map_command_exit_code_to_check_result(*exit_code),
        timestamp: None,
    })
}
