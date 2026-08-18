#!/usr/bin/env bash
# Bring the remote peer up: apply the Terraform, generate a session key, upload
# the binaries (and the ROM, for SFA3) and start the bot.
#
# The session key is generated here and pushed straight into SSM as a
# SecureString. It never touches Terraform state, never appears on a command
# line, and never lands in a file that is not mode 0600.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}"

SIM="${SIM:-arena}"
ROM="${ROM:-}"
PROFILE="${PROFILE:-natural}"
SEED="${SEED:-4242}"
DURATION="${DURATION:-180}"
TF_DIR="${ROOT}/terraform"

if [[ "${SIM}" != "arena" && "${SIM}" != "sfa3" ]]; then
    echo "SIM must be arena or sfa3, got '${SIM}'" >&2
    exit 2
fi
if [[ "${SIM}" == "sfa3" && -z "${ROM}" ]]; then
    echo "SIM=sfa3 needs ROM=/path/to/sfa3.zip" >&2
    exit 2
fi
if [[ ! -f "${TF_DIR}/terraform.tfvars" ]]; then
    echo "terraform/terraform.tfvars is missing." >&2
    echo "Copy terraform/example.tfvars and set allowed_cidr to your own /32:" >&2
    echo "  curl -s https://checkip.amazonaws.com" >&2
    exit 2
fi

echo "==> building release binaries"
cargo build --release -p rollback-bot

echo "==> terraform apply"
terraform -chdir="${TF_DIR}" init -input=false
terraform -chdir="${TF_DIR}" apply -input=false -auto-approve

REGION="$(terraform -chdir="${TF_DIR}" output -raw region)"
BUCKET="$(terraform -chdir="${TF_DIR}" output -raw artifacts_bucket)"
INSTANCE="$(terraform -chdir="${TF_DIR}" output -raw instance_id)"
KEY_PARAM="$(terraform -chdir="${TF_DIR}" output -raw session_key_parameter)"
PEER="$(terraform -chdir="${TF_DIR}" output -raw peer_address)"
PORT="$(terraform -chdir="${TF_DIR}" output -raw session_port)"

echo "==> generating an ephemeral session key"
SESSION_KEY="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
aws ssm put-parameter \
    --region "${REGION}" \
    --name "${KEY_PARAM}" \
    --type SecureString \
    --value "${SESSION_KEY}" \
    --overwrite >/dev/null

# Keep it locally in a root-only file rather than exporting it into every
# subsequent shell's history.
install -d -m 0700 "${ROOT}/artifacts"
umask 077
printf '%s' "${SESSION_KEY}" >"${ROOT}/artifacts/session.key"
echo "    key written to artifacts/session.key (mode 0600) and to SSM"

echo "==> waiting for the instance to finish bootstrapping"
for _ in $(seq 1 60); do
    if aws ssm describe-instance-information \
        --region "${REGION}" \
        --filters "Key=InstanceIds,Values=${INSTANCE}" \
        --query 'InstanceInformationList[0].PingStatus' \
        --output text 2>/dev/null | grep -q Online; then
        break
    fi
    sleep 10
done

echo "==> uploading artefacts to s3://${BUCKET}"
aws s3 cp "${ROOT}/target/release/rollback-bot" "s3://${BUCKET}/bin/rollback-bot" \
    --region "${REGION}" --only-show-errors
if [[ "${SIM}" == "sfa3" ]]; then
    aws s3 cp "${ROOT}/cores/fbneo_libretro.so" "s3://${BUCKET}/bin/fbneo_libretro.so" \
        --region "${REGION}" --only-show-errors
    # The ROM is the user's own file. It goes to an encrypted, private bucket
    # with a seven-day lifecycle, and `just aws-down` deletes it explicitly.
    aws s3 cp "${ROM}" "s3://${BUCKET}/rom/sfa3.zip" \
        --region "${REGION}" --only-show-errors
fi

echo "==> starting the remote peer"
REMOTE_ARGS="--sim ${SIM} --player p2 --bind 0.0.0.0:${PORT} --profile ${PROFILE}"
REMOTE_ARGS="${REMOTE_ARGS} --seed ${SEED} --duration ${DURATION} --mode play"
REMOTE_ARGS="${REMOTE_ARGS} --log-dir /opt/rollback/artifacts/logs"
REMOTE_ARGS="${REMOTE_ARGS} --system-dir /opt/rollback/artifacts/system"
if [[ "${SIM}" == "sfa3" ]]; then
    REMOTE_ARGS="${REMOTE_ARGS} --core /opt/rollback/bin/fbneo_libretro.so"
    REMOTE_ARGS="${REMOTE_ARGS} --rom /opt/rollback/rom/sfa3.zip"
fi

COMMAND_ID="$(aws ssm send-command \
    --region "${REGION}" \
    --instance-ids "${INSTANCE}" \
    --document-name AWS-RunShellScript \
    --comment "start rollback peer" \
    --parameters "commands=[
        'set -euxo pipefail',
        'source /opt/rollback/env',
        'aws s3 cp s3://${BUCKET}/bin/rollback-bot /opt/rollback/bin/rollback-bot',
        'chmod 0755 /opt/rollback/bin/rollback-bot',
        'if [ \"${SIM}\" = sfa3 ]; then mkdir -p /opt/rollback/rom; aws s3 cp s3://${BUCKET}/bin/fbneo_libretro.so /opt/rollback/bin/fbneo_libretro.so; aws s3 cp s3://${BUCKET}/rom/sfa3.zip /opt/rollback/rom/sfa3.zip; fi',
        'printf %s \"#!/usr/bin/env bash\\nset -euo pipefail\\nexport ROLLBACK_SESSION_KEY=\\\$(cat /opt/rollback/secrets/session.key)\\nexec /opt/rollback/bin/rollback-bot ${REMOTE_ARGS}\\n\" > /opt/rollback/bin/run-peer.sh',
        'chmod 0755 /opt/rollback/bin/run-peer.sh',
        'systemctl restart rollback-bot.service',
        'systemctl is-active rollback-bot.service'
    ]" \
    --query 'Command.CommandId' --output text)"

echo "    ssm command ${COMMAND_ID}"
sleep 8
aws ssm get-command-invocation \
    --region "${REGION}" \
    --command-id "${COMMAND_ID}" \
    --instance-id "${INSTANCE}" \
    --query '{status:Status,out:StandardOutputContent,err:StandardErrorContent}' \
    --output json || true

cat <<SUMMARY

==> the remote peer is up

  peer address   ${PEER}
  instance       ${INSTANCE}
  bucket         ${BUCKET}
  shell          aws ssm start-session --region ${REGION} --target ${INSTANCE}

Now, locally:

  export ROLLBACK_SESSION_KEY_FILE=${ROOT}/artifacts/session.key
  just play ${SIM}${ROM:+ ${ROM}}

Remember: 'just collect' BEFORE 'just aws-down'.
SUMMARY
