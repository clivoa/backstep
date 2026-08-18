# 11 — Cleanup

## The order matters

```bash
just collect     # FIRST
just aws-down    # THEN
```

`collect` pulls the remote peer's logs and recordings down from S3. `aws-down`
destroys the bucket. The remote logs exist nowhere else, and the instance goes
away with them.

So `aws-down` refuses to run when `artifacts/logs` is empty:

```
!!! artifacts/logs is empty. 'just collect' has not run.
!!! The remote logs are about to be destroyed with the bucket.
Run 'just collect' first, or re-run with FORCE=1 to discard them.
```

`FORCE=1 just aws-down` discards them deliberately. That is a choice, not an
accident.

## What `aws-down` does

1. Checks that `collect` has run.
2. Empties the bucket explicitly: ROM, BIOS, binaries, remote logs, recordings.
3. `terraform destroy -auto-approve`.
4. Deletes the local copy of the session key (`artifacts/session.key`).

Step 2 is redundant with `force_destroy = true` and exists anyway. The bucket
holds somebody's ROM, and leaving it behind in an AWS account is not acceptable.
A `terraform destroy` that fails for any reason should not leave the ROM sitting
there.

## Confirming it is gone

`aws-down` prints these at the end. Run them.

```bash
REGION=eu-central-1

# No live instances
aws ec2 describe-instances --region $REGION \
  --filters Name=tag:Project,Values=rollback-netcode \
            Name=instance-state-name,Values=running,pending,stopping,stopped \
  --query 'Reservations[].Instances[].InstanceId'

# No bucket
aws s3 ls | grep rollback-netcode

# No idle Elastic IP  <-- this is the one that costs money
aws ec2 describe-addresses --region $REGION \
  --query 'Addresses[?AssociationId==`null`].[PublicIp,AllocationId]'

# No orphaned volumes
aws ec2 describe-volumes --region $REGION \
  --filters Name=status,Values=available \
  --query 'Volumes[].[VolumeId,Size]'

# No lab VPC
aws ec2 describe-vpcs --region $REGION \
  --filters Name=tag:Project,Values=rollback-netcode \
  --query 'Vpcs[].VpcId'

# The key parameter
aws ssm get-parameter --region $REGION --name /rollback-netcode/session-key \
  2>&1 | head -1
```

All should come back empty, or `ParameterNotFound` for the last one.

Every resource carries the tag `Project=rollback-netcode`, so that is the search
that finds anything left behind.

## If the destroy fails partway

The usual failure is Terraform being unable to delete the VPC because something
is still attached to it, an ENI or an EIP. Run it again:

```bash
terraform -chdir=terraform destroy -auto-approve
```

If it persists, what tends to survive, in order of cost:

1. An unattached Elastic IP, about US$ 3.60/month.
   `aws ec2 release-address --region eu-central-1 --allocation-id eipalloc-...`
2. An available EBS volume. `aws ec2 delete-volume --volume-id vol-...`
3. A network interface.
   `aws ec2 delete-network-interface --network-interface-id eni-...`
4. A bucket with objects. `aws s3 rb s3://... --force`

Then run `terraform destroy` once more so the state is consistent.

## Local cleanup

```bash
just clean-logs   # deletes artifacts/logs, artifacts/report, artifacts/e2e
just clean        # the above plus cargo clean
just elastic-down # if the Elastic stack is up
just local-down   # if Prometheus and Grafana are up
```

Two things survive on purpose:

`cores/fbneo_libretro.so` takes half an hour to rebuild and is not deleted by
accident. Remove it by hand if you want to.

`terraform/terraform.tfstate` orphans everything if you delete it while
resources are alive. Only remove it after confirming the destroy finished.

Recordings under `artifacts/video/` are not touched by `clean-logs` either. A
five-profile run is about a gigabyte, so delete them when you are done looking.

## What was never in the repository

ROMs and the BIOS. `*.zip` is gitignored and no step of the lab copies a ROM
into the source tree.

Savestates. All of `artifacts/` is ignored.

Keys. `session-key*` and `*.tfvars` (except `example.tfvars`) are ignored.

Personal reports and recordings. They live under `artifacts/`, ignored.

Worth checking before publishing anything:

```bash
git status --porcelain --ignored | grep '^!!' | head -20
```

For a stronger check before pushing somewhere public, search the whole history
rather than the working tree:

```bash
git log --all --pretty=format: --name-only --diff-filter=A | sort -u \
  | grep -iE '\.zip$|\.so$|\.mp4$|session\.key|tfstate'
```

## End-of-session checklist

- [ ] `just collect` ran, and `artifacts/logs` has files from **both** peers
- [ ] `just aws-down` finished without error
- [ ] The six verification commands above all return empty
- [ ] `artifacts/session.key` no longer exists
- [ ] `just local-down` and `just elastic-down`, if either stack is up
