use k8s_openapi::api::batch::v1::Job;
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
