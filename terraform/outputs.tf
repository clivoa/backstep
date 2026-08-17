output "peer_address" {
  description = "What to pass to `rollback-client --peer`."
  value       = "${aws_eip.peer.public_ip}:${var.session_port}"
}

output "peer_public_ip" {
  description = "Elastic IP of the remote peer."
  value       = aws_eip.peer.public_ip
}

output "instance_id" {
  description = "Instance id, for `aws ssm start-session`."
  value       = aws_instance.peer.id
}

output "instance_type" {
  description = "Instance type actually launched."
  value       = aws_instance.peer.instance_type
}

output "region" {
  value = var.region
}

output "artifacts_bucket" {
  description = "S3 bucket holding the ROM, binaries and collected logs."
  value       = aws_s3_bucket.artifacts.bucket
}

output "session_key_parameter" {
  description = "SSM SecureString parameter holding the session key. The value is never in Terraform state."
  value       = aws_ssm_parameter.session_key.name
}

output "session_port" {
  value = var.session_port
}

output "allowed_cidr" {
  description = "The only CIDR that can reach the game port."
  value       = var.allowed_cidr
}

output "auto_shutdown_hours" {
  description = "Hours after boot at which the instance terminates itself."
  value       = var.auto_shutdown_hours
}

output "ssm_session_command" {
  description = "Copy-paste to get a shell without SSH."
  value       = "aws ssm start-session --region ${var.region} --target ${aws_instance.peer.id}"
}
