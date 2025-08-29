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
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;

use tokio::{sync::mpsc, time::Instant};

use crate::check::SharedRunnableChecks;
use crate::check::{self, RunnableCheck, ScheduledCheck};
use crate::job::build_bash_job;
use crate::job::build_python_job;
use crate::job::is_job_finished;

pub enum RunnableCheckEvent {
    AddOrUpdate(Arc<RunnableCheck>),
    Remove(String), // check_name
}

/**
 * This function handles the addition/update and removal of checks when an events on the Kube side occurs
 */
async fn handle_check_event(
    event: RunnableCheckEvent,
    queue: &mut BTreeMap<String, ScheduledCheck>,
    shared_checks: SharedRunnableChecks,
) {
    match event {
        RunnableCheckEvent::AddOrUpdate(check) => {
            let check_name = check.check_name.clone();
            let next_run = Instant::now() + Duration::from_secs(check.interval);

            shared_checks
                .write()
                .await
                .insert(check_name.clone(), check.clone());

            // Update the scheduled check
            queue.remove(&check.check_name);
            queue.insert(check.check_name.clone(), ScheduledCheck { next_run, check });
        }
        RunnableCheckEvent::Remove(check_name) => {
            shared_checks.write().await.remove(&check_name);
            queue.remove(&check_name);
        }
    }
}

/**
 * This function continuously schedule checks based on the interval
 */
pub async fn scheduler_loop(
    mut event_rx: mpsc::Receiver<RunnableCheckEvent>,
    result_tx: mpsc::Sender<CheckResult>,
    shared_checks: SharedRunnableChecks,
    namespace: String,
) {
    let mut queue: BTreeMap<String, ScheduledCheck> = BTreeMap::new();

    // Continuosly loop
    loop {
        // Check if there's a scheduled task
        if let Some((_check_name, mut scheduled_check)) =
            queue.iter().next().map(|(k, v)| (k.clone(), v.clone()))
        {
            let now = Instant::now();
            let delay = scheduled_check.next_run.saturating_duration_since(now);

            select! {
                maybe_event = event_rx.recv() => {
                    if let Some(event) = maybe_event {
                        handle_check_event(event, &mut queue, shared_checks.clone()).await
                    }
                }
                _ = tokio::time::sleep(delay) => {
                    // Check still valid? (not removed)
                    if !shared_checks
                        .read()
                        .await
                        .contains_key(&scheduled_check.check.check_name)
                    {
                        continue; // Skip deleted check
                    }

                    let check_interval = Duration::from_secs(scheduled_check.check.interval);

                    // Run the check asynchronously
                    let tx = result_tx.clone();
                    tokio::spawn(run_check(scheduled_check.check.clone(), tx, namespace.clone()));

                    // Schedule the next run
                    scheduled_check.next_run += check_interval;
                    queue.insert(scheduled_check.check.check_name.clone(), scheduled_check);
                }
            }
        } else {
            // No scheduled checks, wait for events
            if let Some(event) = event_rx.recv().await {
                handle_check_event(event, &mut queue, shared_checks.clone()).await
            }
        }
    }
}

/**
 * This function runs a check, parses the result to a check result and returns it to the main thread
 */
pub async fn run_check(
    check: Arc<RunnableCheck>,
    result_tx: mpsc::Sender<CheckResult>,
    namespace: String,
) {
    // Run the check as Kubernetes job
    // TODO: One kube job for check execution is probably overkill, are there better alternatives?
    let mut check_result = match run_check_as_kube_job(check, namespace).await {
        Ok(res) | Err(res) => res,
    };

    // Inject the timestamp of when we completed the check execution
    let timestamp = Local::now();
    let timestamp_utc = timestamp.with_timezone(&Utc);
    check_result.set_check_result_timestamp(timestamp_utc);

    // Send the check result
    if let Err(e) = result_tx.send(check_result).await {
        error!("Error sending check result: {e:?}")
    }
}

/**
 * This function runs a check as a Kubernetes job
 */
async fn run_check_as_kube_job(
    check: Arc<RunnableCheck>,
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
            format!("Error when creating the Kubernetes client: {e:?}"),
        )
    })?;

    // Build the job
    let job = match check.language {
        check::ScriptLanguage::Python => build_python_job(
            &job_name,
            &check.script,
            &check.secrets_refs,
            &check.python_requirements,
        ),
        check::ScriptLanguage::Bash => {
            build_bash_job(&job_name, &check.script, &check.secrets_refs)
        }
    };

    // Get the job API
    let jobs: Api<Job> = Api::namespaced(client.clone(), &namespace);

    debug!("Creating job for check {check_name}");

    // Create the job
    jobs.create(&PostParams::default(), &job)
        .await
        .map_err(|e| {
            CheckResult::map_to_check_error(
                check_name,
                format!("Error when creating the Kubernetes job: {e:?}"),
            )
        })?;

    // Wait for the job to complete
    debug!("Waiting for job {job_name} for check {check_name} to complete...",);
    let _ = await_condition(jobs.clone(), &job_name, is_job_finished()).await;

    // Get the Pod created by the Job
    let pods: Api<Pod> = Api::namespaced(client.clone(), &namespace);
    let lp = ListParams::default().labels(&format!("job-name={job_name}"));
    let pod_list = pods.list(&lp).await.map_err(|e| {
        CheckResult::map_to_check_error(
            check_name,
            format!("Error when retrieving the pods list: {e:?}"),
        )
    })?;

    let pod = pod_list.items.into_iter().next().ok_or_else(|| {
        CheckResult::map_to_check_error(check_name, format!("Cannot find pod for job {job_name}"))
    })?;

    let pod_name = pod.metadata.name.clone().ok_or_else(|| {
        CheckResult::map_to_check_error(
            check_name,
            format!("Error getting pod name from pod {pod:?}"),
        )
    })?;

    // Get the pod logs
    let logs = pods
        .logs(&pod_name, &Default::default())
        .await
        .map_err(|e| {
            CheckResult::map_to_check_error(check_name, format!("Cannot get pod logs: {e:?}"))
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
        error!("Error when deleting job after completion: {job_name:?} - {e:?}")
    }

    Ok(CheckResult {
        check_name: check_name.to_string(),
        output: logs,
        status: map_command_exit_code_to_check_result(*exit_code),
        timestamp: None,
        telegram_channels: check.telegram_channels.clone().into(),
    })
}
