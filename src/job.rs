use k8s_openapi::api::{
    batch::v1::Job,
    core::v1::{EnvFromSource, SecretEnvSource},
};
use kube::runtime::wait::Condition;

pub fn is_job_finished() -> impl Condition<Job> {
    |job: Option<&Job>| {
        if let Some(job) = job {
            if let Some(status) = &job.status {
                let failed = status.failed.unwrap_or(0) > 0;
                let succeeded = status.succeeded.unwrap_or(0) > 0;
                return failed || succeeded;
            }
        }
        false
    }
}

/**
 * This function takes the script Bash code and creates a kubernetes job to run it
 */
pub fn build_bash_job(
    job_name: &str,
    check_script: &str,
    secrets_refs: &Option<Vec<String>>,
) -> Job {
    // Escape newlines and quotes to run inline in bash -c
    let escaped_script = check_script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("; ");

    let escaped_script = format!("set -euo pipefail; {escaped_script}");

    // Create the secrets object, if needed
    let env_from: Option<Vec<EnvFromSource>> = secrets_refs.as_ref().map(|secret_names| {
        secret_names
            .iter()
            .map(|secret_name| EnvFromSource {
                secret_ref: Some(SecretEnvSource {
                    name: Some(secret_name.clone()),
                    optional: Some(false),
                }),
                ..Default::default()
            })
            .collect()
    });

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
                        env_from,
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

// This function taks the script Python code and creates a Kubernetes job to run it
pub fn build_python_job(
    job_name: &str,
    check_script: &str,
    secrets_refs: &Option<Vec<String>>,
    requirements: &Option<Vec<String>>,
) -> Job {
    let pip_command = if let Some(requirements) = requirements {
        let reqs = requirements.join(" ");

        format!("pip install {reqs}")
    } else {
        "".to_string()
    };

    let command = format!(
        "{pip} > pip.log 2>&1 || (cat pip.log && exit 4) && python <<'EOF'\n{code}\nEOF",
        pip = pip_command,
        code = check_script.trim()
    );

    // Create the secrets object, if needed
    let env_from: Option<Vec<EnvFromSource>> = secrets_refs.as_ref().map(|secret_names| {
        secret_names
            .iter()
            .map(|secret_name| EnvFromSource {
                secret_ref: Some(SecretEnvSource {
                    name: Some(secret_name.clone()),
                    optional: Some(false),
                }),
                ..Default::default()
            })
            .collect()
    });

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
                        name: "python-script".to_string(),
                        image: Some("python:3.11-slim".into()),
                        command: Some(vec!["bash".into(), "-c".into()]),
                        args: Some(vec![command]),
                        env_from,
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
