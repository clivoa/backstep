#!/usr/bin/env bash
# Pull the remote peer's logs down and build the report.
#
# Must run BEFORE `just aws-down`: the teardown destroys the bucket and the
# instance, and there is no second copy of the remote logs anywhere.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}"

TF_DIR="${ROOT}/terraform"
LOG_DIR="${ROOT}/artifacts/logs"
REPORT_DIR="${ROOT}/artifacts/report"

if [[ ! -d "${TF_DIR}/.terraform" ]]; then
    echo "Terraform has not been initialised; is anything actually up?" >&2
    exit 2
fi

REGION="$(terraform -chdir="${TF_DIR}" output -raw region)"
BUCKET="$(terraform -chdir="${TF_DIR}" output -raw artifacts_bucket)"
INSTANCE="$(terraform -chdir="${TF_DIR}" output -raw instance_id)"

mkdir -p "${LOG_DIR}" "${REPORT_DIR}"

echo "==> asking the remote peer to flush its logs"
aws ssm send-command \
    --region "${REGION}" \
    --instance-ids "${INSTANCE}" \
    --document-name AWS-RunShellScript \
    --comment "flush rollback logs" \
    --parameters 'commands=["/usr/local/bin/rollback-sync-logs"]' \
    --query 'Command.CommandId' --output text >/dev/null || \
    echo "    (the instance did not answer; falling back to whatever is already in S3)"

sleep 5

echo "==> downloading from s3://${BUCKET}/remote"
aws s3 sync "s3://${BUCKET}/remote/artifacts/video" "${ROOT}/artifacts/video/remote" \
    --region "${REGION}" --only-show-errors || true
# Count what is already here. A run whose logs get overwritten is a run that
# has to be paid for again, and the numbers in the docs stop being checkable.
BEFORE="$(find "${LOG_DIR}" -name '*.jsonl' 2>/dev/null | wc -l)"

aws s3 sync "s3://${BUCKET}/remote/artifacts/logs" "${LOG_DIR}" \
    --region "${REGION}" --only-show-errors
aws s3 cp "s3://${BUCKET}/remote/bootstrap.log" "${ROOT}/artifacts/remote-bootstrap.log" \
    --region "${REGION}" --only-show-errors 2>/dev/null || true

echo "==> building the report"
cargo build --release -p rollback-report
"${ROOT}/target/release/rollback-report" --logs "${LOG_DIR}" --out "${REPORT_DIR}"

AFTER="$(find "${LOG_DIR}" -name '*.jsonl' | wc -l)"

echo
echo "==> collected (${BEFORE} session log(s) were already here, ${AFTER} now)"
find "${LOG_DIR}" -name '*.jsonl' -printf '  %f  %s bytes\n' | sort
echo
echo "  ${REPORT_DIR}/summary.csv"
echo "  ${REPORT_DIR}/report.html"
echo
echo "Safe to run 'just aws-down' now."
