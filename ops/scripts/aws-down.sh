#!/usr/bin/env bash
# Destroy everything: instance, volume, bucket contents, ROM, remote logs, key.
#
# Deliberately thorough. The bucket holds the user's ROM and their session logs;
# leaving either behind in someone's AWS account is not acceptable, so the
# objects are deleted explicitly before `terraform destroy` rather than relying
# on force_destroy alone.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TF_DIR="${ROOT}/terraform"

if [[ ! -d "${TF_DIR}/.terraform" ]]; then
    echo "Nothing to tear down: terraform has not been initialised."
    exit 0
fi

REGION="$(terraform -chdir="${TF_DIR}" output -raw region 2>/dev/null || echo "")"
BUCKET="$(terraform -chdir="${TF_DIR}" output -raw artifacts_bucket 2>/dev/null || echo "")"

# Guard against tearing down before collecting: the remote logs only exist in
# the bucket, and this is the last moment they can be saved.
if [[ ! -d "${ROOT}/artifacts/logs" || -z "$(ls -A "${ROOT}/artifacts/logs" 2>/dev/null)" ]]; then
    echo "!!! artifacts/logs is empty. 'just collect' has not run." >&2
    echo "!!! The remote logs are about to be destroyed with the bucket." >&2
    if [[ "${FORCE:-0}" != "1" ]]; then
        echo "Run 'just collect' first, or re-run with FORCE=1 to discard them." >&2
        exit 2
    fi
    echo "FORCE=1: discarding the remote logs." >&2
fi

if [[ -n "${BUCKET}" && -n "${REGION}" ]]; then
    echo "==> emptying s3://${BUCKET} (ROM, binaries, remote logs)"
    aws s3 rm "s3://${BUCKET}" --recursive --region "${REGION}" --only-show-errors || true
fi

echo "==> terraform destroy"
terraform -chdir="${TF_DIR}" destroy -input=false -auto-approve

# The local copy of the session key is useless now, and a key on disk with no
# session behind it is only a liability.
rm -f "${ROOT}/artifacts/session.key"

echo
echo "==> torn down. Verify nothing is left:"
echo "  aws ec2 describe-instances --region ${REGION:-eu-central-1} \\"
echo "    --filters Name=tag:Project,Values=rollback-netcode Name=instance-state-name,Values=running,pending,stopped \\"
echo "    --query 'Reservations[].Instances[].InstanceId'"
echo "  aws s3 ls | grep rollback-netcode"
