#!/usr/bin/env bash
# Bring the remote peer up: apply the Terraform, generate a session key, upload
# the binaries (and the ROM, and the BIOS if the game needs one) and start the
# bot.
#
# The session key is generated here and pushed straight into SSM as a
# SecureString. It never touches Terraform state, never appears on a command
# line, and never lands in a file that is not mode 0600.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}"

SIM="${SIM:-arena}"
ROM="${ROM:-}"
# Neo Geo games are only half the code that runs; neogeo.zip is the other half,
# and it is hashed into the handshake. Both peers must have the same file.
BIOS="${BIOS:-${ROOT}/artifacts/system/neogeo.zip}"
PROFILE="${PROFILE:-natural}"
SEED="${SEED:-4242}"
DURATION="${DURATION:-180}"
# Goes into the session log's filename, so runs from different scenarios never
# look alike on disk. Losing a run to an ambiguous filename costs a rebuild of
# the whole session; a label costs nothing.
MODE="${MODE:-play}"
# How long the remote peer waits for the local one before giving up.
#
# The bot's own default of 120 s suits `bench`, where a script launches both
# peers seconds apart. A human session is different: `aws-up` finishes, then a
# person has to read the output, find the window and sit down. Two minutes of
# that leaves an instance up and billing whose peer has already exited -- it
# still answers SSM and still shows a listening socket in `systemctl status`
# right up until it does not, so the failure reads as a handshake that hangs
# for no reason. Fifteen minutes by default; the auto-shutdown still bounds it.
HANDSHAKE_TIMEOUT="${HANDSHAKE_TIMEOUT:-900}"
# These are part of the handshake's configuration hash, so the remote peer must
# be told the same values or the session is refused before it starts.
PREDICTION_LIMIT="${PREDICTION_LIMIT:-8}"
INPUT_DELAY="${INPUT_DELAY:-1}"
STATE_HISTORY="${STATE_HISTORY:-16}"
TF_DIR="${ROOT}/terraform"

case "${SIM}" in
    arena) NEEDS_ROM=0; NEEDS_BIOS=0 ;;
    sfa3) NEEDS_ROM=1; NEEDS_BIOS=0 ;;
    lastblade2) NEEDS_ROM=1; NEEDS_BIOS=1 ;;
    *) echo "SIM must be arena, sfa3 or lastblade2, got '${SIM}'" >&2; exit 2 ;;
esac

if [[ ${NEEDS_ROM} -eq 1 && -z "${ROM}" ]]; then
    echo "SIM=${SIM} needs ROM=/path/to/game.zip" >&2
    exit 2
fi
if [[ ${NEEDS_ROM} -eq 1 && ! -f "${ROM}" ]]; then
    echo "no ROM at '${ROM}'" >&2
    exit 2
fi
if [[ ${NEEDS_BIOS} -eq 1 && ! -f "${BIOS}" ]]; then
    echo "SIM=${SIM} needs the Neo Geo BIOS at '${BIOS}'." >&2
    echo "Both peers must use the same file: it is hashed into the handshake." >&2
    exit 2
fi
# The remote peer must be handed the ROM under the name FBNeo expects, because
# the romset name is the zip's basename.
ROM_NAME="$(basename "${ROM:-none}")"
if [[ ! -f "${TF_DIR}/terraform.tfvars" ]]; then
    echo "terraform/terraform.tfvars is missing." >&2
    echo "Copy terraform/example.tfvars and set allowed_cidr to your own /32:" >&2
    echo "  curl -s https://checkip.amazonaws.com" >&2
    exit 2
fi

if [[ ${RECORD:-0} -eq 1 ]]; then
    echo "==> the remote peer will record video (needs ffmpeg on the instance)"
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

echo "==> waiting for the SSM agent"
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

# The SSM agent answers long before cloud-init has finished -- it is baked into
# the AMI and starts during boot, while user_data is still downloading and
# installing the AWS CLI. Sending the start command on the strength of a ping
# raced the bootstrap and failed with "cannot open /opt/rollback/env", which
# reads like a broken script rather than "you were early".
#
# So wait for the marker the bootstrap writes when it is genuinely done.
echo "==> waiting for the bootstrap to finish"
BOOTSTRAPPED=0
for _ in $(seq 1 60); do
    MARKER_CMD="$(aws ssm send-command \
        --region "${REGION}" \
        --instance-ids "${INSTANCE}" \
        --document-name AWS-RunShellScript \
        --comment "await bootstrap" \
        --parameters 'commands=["test -f /opt/rollback/BOOTSTRAP_COMPLETE"]' \
        --query 'Command.CommandId' --output text 2>/dev/null || true)"
    if [[ -n "${MARKER_CMD}" ]]; then
        sleep 5
        if aws ssm get-command-invocation \
            --region "${REGION}" --command-id "${MARKER_CMD}" --instance-id "${INSTANCE}" \
            --query 'Status' --output text 2>/dev/null | grep -q Success; then
            BOOTSTRAPPED=1
            break
        fi
    fi
    sleep 5
done

if [[ ${BOOTSTRAPPED} -ne 1 ]]; then
    echo "!!! the instance never finished bootstrapping." >&2
    echo "!!! look at the log: aws ssm start-session --region ${REGION} --target ${INSTANCE}" >&2
    echo "!!!   sudo tail -50 /var/log/rollback-bootstrap.log" >&2
    exit 1
fi
echo "    bootstrap complete"

echo "==> uploading artefacts to s3://${BUCKET}"
aws s3 cp "${ROOT}/target/release/rollback-bot" "s3://${BUCKET}/bin/rollback-bot" \
    --region "${REGION}" --only-show-errors
if [[ ${NEEDS_ROM} -eq 1 ]]; then
    aws s3 cp "${ROOT}/cores/fbneo_libretro.so" "s3://${BUCKET}/bin/fbneo_libretro.so" \
        --region "${REGION}" --only-show-errors
    # The ROM is the user's own file. It goes to an encrypted, private bucket
    # with a seven-day lifecycle, and `just aws-down` deletes it explicitly.
    aws s3 cp "${ROM}" "s3://${BUCKET}/rom/${ROM_NAME}" \
        --region "${REGION}" --only-show-errors
fi
if [[ ${NEEDS_BIOS} -eq 1 ]]; then
    aws s3 cp "${BIOS}" "s3://${BUCKET}/rom/neogeo.zip" \
        --region "${REGION}" --only-show-errors
fi

echo "==> starting the remote peer"
REMOTE_ARGS="--sim ${SIM} --player p2 --bind 0.0.0.0:${PORT} --profile ${PROFILE}"
REMOTE_ARGS="${REMOTE_ARGS} --seed ${SEED} --duration ${DURATION} --mode ${MODE}"
REMOTE_ARGS="${REMOTE_ARGS} --prediction-limit ${PREDICTION_LIMIT} --input-delay ${INPUT_DELAY}"
REMOTE_ARGS="${REMOTE_ARGS} --state-history ${STATE_HISTORY}"
REMOTE_ARGS="${REMOTE_ARGS} --handshake-timeout ${HANDSHAKE_TIMEOUT}"
REMOTE_ARGS="${REMOTE_ARGS} --log-dir /opt/rollback/artifacts/logs"
REMOTE_ARGS="${REMOTE_ARGS} --system-dir /opt/rollback/artifacts/system"
if [[ ${RECORD:-0} -eq 1 && ${NEEDS_ROM} -eq 1 ]]; then
    # The far side's video is the whole point of recording a real link: the two
    # peers do wildly different amounts of work, and only one of them is here.
    REMOTE_ARGS="${REMOTE_ARGS} --record /opt/rollback/artifacts/video/peer.mp4"
fi
if [[ ${NEEDS_ROM} -eq 1 ]]; then
    REMOTE_ARGS="${REMOTE_ARGS} --core /opt/rollback/bin/fbneo_libretro.so"
    REMOTE_ARGS="${REMOTE_ARGS} --rom /opt/rollback/rom/${ROM_NAME}"
fi

# Build the launcher HERE and ship it, rather than writing it with a printf
# inside an SSM command.
#
# That printf was three layers of quoting deep -- bash string, JSON string,
# remote sh -- and it silently lost: `printf %s` does not interpret \n, so the
# whole script landed on one line and the remote spent minutes trying to exec a
# program whose name began "bash\nset -euo pipefail". The service reported
# active the entire time.
#
# A file uploaded to S3 has no escaping at all.
LAUNCHER="$(mktemp)"
trap 'rm -f "${LAUNCHER}"' EXIT
cat >"${LAUNCHER}" <<LAUNCH
#!/usr/bin/env bash
set -euo pipefail
export ROLLBACK_SESSION_KEY="\$(tr -d '\\n' < /opt/rollback/secrets/session.key)"
exec /opt/rollback/bin/rollback-bot ${REMOTE_ARGS}
LAUNCH
aws s3 cp "${LAUNCHER}" "s3://${BUCKET}/bin/run-peer.sh" \
    --region "${REGION}" --only-show-errors

COMMAND_ID="$(aws ssm send-command \
    --region "${REGION}" \
    --instance-ids "${INSTANCE}" \
    --document-name AWS-RunShellScript \
    --comment "start rollback peer" \
    --parameters "commands=[
        'set -eux',
        '. /opt/rollback/env',
        'aws s3 cp --only-show-errors s3://${BUCKET}/bin/rollback-bot /opt/rollback/bin/rollback-bot',
        'aws s3 cp --only-show-errors s3://${BUCKET}/bin/run-peer.sh /opt/rollback/bin/run-peer.sh',
        'chmod 0755 /opt/rollback/bin/rollback-bot /opt/rollback/bin/run-peer.sh',
        'if [ ${NEEDS_ROM} -eq 1 ]; then mkdir -p /opt/rollback/rom; aws s3 cp --only-show-errors s3://${BUCKET}/bin/fbneo_libretro.so /opt/rollback/bin/fbneo_libretro.so; aws s3 cp --only-show-errors s3://${BUCKET}/rom/${ROM_NAME} /opt/rollback/rom/${ROM_NAME}; fi',
        'mkdir -p /opt/rollback/artifacts/video',
        'if [ ${NEEDS_BIOS} -eq 1 ]; then mkdir -p /opt/rollback/artifacts/system; aws s3 cp --only-show-errors s3://${BUCKET}/rom/neogeo.zip /opt/rollback/artifacts/system/neogeo.zip; fi',
        'head -1 /opt/rollback/bin/run-peer.sh',
        'systemctl restart rollback-bot.service',
        'sleep 3',
        'systemctl is-active rollback-bot.service',
        'ss -lun | grep -q :${PORT} || (journalctl -u rollback-bot.service -n 20 --no-pager; echo NO-UDP-LISTENER; exit 1)'
    ]" \
    --query 'Command.CommandId' --output text)"

echo "    ssm command ${COMMAND_ID}"

# Wait for a terminal status rather than guessing with a sleep.
STATUS="Pending"
for _ in $(seq 1 30); do
    sleep 4
    STATUS="$(aws ssm get-command-invocation \
        --region "${REGION}" --command-id "${COMMAND_ID}" --instance-id "${INSTANCE}" \
        --query 'Status' --output text 2>/dev/null || echo Pending)"
    case "${STATUS}" in
        Success | Failed | Cancelled | TimedOut) break ;;
    esac
done

aws ssm get-command-invocation \
    --region "${REGION}" \
    --command-id "${COMMAND_ID}" \
    --instance-id "${INSTANCE}" \
    --query '{status:Status,out:StandardOutputContent,err:StandardErrorContent}' \
    --output json || true

# Do not print "the peer is up" when it is not. The first version of this script
# did, and the operator only found out when the handshake timed out sixty
# seconds later with no explanation.
if [[ "${STATUS}" != "Success" ]]; then
    echo >&2
    echo "!!! the remote peer did NOT start (ssm status: ${STATUS})" >&2
    echo "!!! the infrastructure is up and costing money. Either fix and re-run" >&2
    echo "!!! this script, or tear it down with 'just aws-down'." >&2
    exit 1
fi

# Record what the remote peer was actually started with, so the local side can
# agree with it by construction instead of by both hard-coding the same numbers.
#
# The handshake hashes simulation, seed, tick rate, input delay, prediction
# limit and state history together, and refuses the session if a single one
# differs. Before this file existed, `just play` passed none of them and used
# the client's built-in defaults, so `just aws-up` followed by `just play` --
# the documented path -- always failed with "session configuration mismatch".
# It went unnoticed for as long as it did because every session until the first
# human one had both peers launched by the same script.
SESSION_ENV="${ROOT}/artifacts/session.env"
cat > "${SESSION_ENV}" <<ENV
# Written by aws-up.sh. Read by 'just play'. Do not edit by hand: every value
# here has to match what the remote peer was started with.
ROLLBACK_PEER=${PEER}
ROLLBACK_SIM=${SIM}
ROLLBACK_PROFILE=${PROFILE}
ROLLBACK_SEED=${SEED}
ROLLBACK_INPUT_DELAY=${INPUT_DELAY}
ROLLBACK_PREDICTION_LIMIT=${PREDICTION_LIMIT}
ROLLBACK_STATE_HISTORY=${STATE_HISTORY}
ROLLBACK_MODE=${MODE}
ENV
chmod 0600 "${SESSION_ENV}"

cat <<SUMMARY

==> the remote peer is up

  peer address   ${PEER}
  instance       ${INSTANCE}
  bucket         ${BUCKET}
  shell          aws ssm start-session --region ${REGION} --target ${INSTANCE}
  config         seed=${SEED} input_delay=${INPUT_DELAY} prediction_limit=${PREDICTION_LIMIT} state_history=${STATE_HISTORY}
  waiting        ${HANDSHAKE_TIMEOUT}s for you to connect

Now, locally:

  just play ${SIM}${ROM:+ ${ROM}}

'just play' reads artifacts/session.env, so it agrees with the remote peer on
every field the handshake checks. If you would rather run the client yourself:

  export ROLLBACK_SESSION_KEY_FILE=${ROOT}/artifacts/session.key
  ./target/release/rollback-client --sim ${SIM} --peer ${PEER} \
    --profile ${PROFILE} --seed ${SEED} --input-delay ${INPUT_DELAY} \
    --prediction-limit ${PREDICTION_LIMIT} --state-history ${STATE_HISTORY} \
    --mode ${MODE} --log-dir ${ROOT}/artifacts/logs${ROM:+ \\
    --core ${ROOT}/cores/fbneo_libretro.so --rom ${ROM} --system-dir ${ROOT}/artifacts/system}

Remember: 'just collect' BEFORE 'just aws-down'.
SUMMARY
