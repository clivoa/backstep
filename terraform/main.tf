# Infrastructure for the remote peer.
#
# One instance in one public subnet, reachable on exactly one UDP port from
# exactly one address, administered through SSM so there is no SSH key and no
# port 22. Everything is tagged with the project name and everything is
# destroyed by `just aws-down`.

locals {
  name = var.project

  tags = {
    Project   = var.project
    ManagedBy = "terraform"
    Purpose   = "rollback-netcode-lab"
  }
}

data "aws_availability_zones" "available" {
  state = "available"
}

# Canonical's official Ubuntu 24.04 LTS, x86_64. Looked up rather than pinned to
# an AMI id: AMI ids are region-specific and Canonical republishes them, so a
# hard-coded id rots quietly. The owner id is the pin that matters.
data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical

  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*"]
  }

  filter {
    name   = "virtualization-type"
    values = ["hvm"]
  }

  filter {
    name   = "architecture"
    values = ["x86_64"]
  }
}

# --- network ---------------------------------------------------------------

resource "aws_vpc" "lab" {
  cidr_block           = "10.42.0.0/16"
  enable_dns_support   = true
  enable_dns_hostnames = true

  tags = { Name = "${local.name}-vpc" }
}

resource "aws_internet_gateway" "lab" {
  vpc_id = aws_vpc.lab.id
  tags   = { Name = "${local.name}-igw" }
}

resource "aws_subnet" "public" {
  vpc_id                  = aws_vpc.lab.id
  cidr_block              = "10.42.1.0/24"
  availability_zone       = data.aws_availability_zones.available.names[0]
  map_public_ip_on_launch = true

  tags = { Name = "${local.name}-public" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.lab.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.lab.id
  }

  tags = { Name = "${local.name}-public" }
}

resource "aws_route_table_association" "public" {
  subnet_id      = aws_subnet.public.id
  route_table_id = aws_route_table.public.id
}

# --- security --------------------------------------------------------------

# Inbound: the game port, from one address. That is the entire attack surface.
#
# There is no SSH rule on purpose. Administration goes through SSM Session
# Manager, which needs no inbound rule at all -- the agent dials out. A lab that
# opens 22 "just for debugging" is a lab with a permanent hole in it.
resource "aws_security_group" "peer" {
  name        = "${local.name}-peer"
  description = "Rollback session peer: UDP game port only"
  vpc_id      = aws_vpc.lab.id

  tags = { Name = "${local.name}-peer" }
}

resource "aws_vpc_security_group_ingress_rule" "session" {
  security_group_id = aws_security_group.peer.id
  # No apostrophe: AWS rejects rule descriptions outside
  # [a-zA-Z0-9._-:/()#,@[]+=&;{}!$*] and space, and "operator's" fails the
  # apply with a bare InvalidParameterValue. Only a real apply finds this --
  # `terraform validate` is perfectly happy with it.
  description = "Rollback session traffic from the operator address"
  cidr_ipv4   = var.allowed_cidr
  from_port   = var.session_port
  to_port     = var.session_port
  ip_protocol = "udp"
}

# Egress is open: the instance has to reach S3, SSM and the package mirrors.
resource "aws_vpc_security_group_egress_rule" "all" {
  security_group_id = aws_security_group.peer.id
  description       = "Outbound to AWS APIs, package mirrors and the peer"
  cidr_ipv4         = "0.0.0.0/0"
  ip_protocol       = "-1"
}

# --- artefact storage ------------------------------------------------------

resource "random_id" "bucket_suffix" {
  byte_length = 4
}

resource "aws_s3_bucket" "artifacts" {
  bucket = "${local.name}-${random_id.bucket_suffix.hex}"

  # The bucket holds a ROM, session logs and build artefacts. None of it is
  # meant to outlive the experiment, so let `terraform destroy` empty it rather
  # than fail on a non-empty bucket and leave the account dirty.
  force_destroy = true

  tags = { Name = "${local.name}-artifacts" }
}

resource "aws_s3_bucket_public_access_block" "artifacts" {
  bucket                  = aws_s3_bucket.artifacts.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_server_side_encryption_configuration" "artifacts" {
  bucket = aws_s3_bucket.artifacts.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_versioning" "artifacts" {
  bucket = aws_s3_bucket.artifacts.id

  versioning_configuration {
    status = "Suspended"
  }
}

# Belt and braces with `just aws-down`: even if a teardown is forgotten, the
# ROM and logs age out on their own.
resource "aws_s3_bucket_lifecycle_configuration" "artifacts" {
  bucket = aws_s3_bucket.artifacts.id

  rule {
    id     = "expire-lab-artifacts"
    status = "Enabled"

    filter {}

    expiration {
      days = var.log_retention_days
    }

    abort_incomplete_multipart_upload {
      days_after_initiation = 1
    }
  }
}

# --- session key -----------------------------------------------------------

# The HMAC key both peers authenticate with.
#
# Generated by AWS, not by Terraform: `random_password` would put the value in
# the state file in clear. Here Terraform only creates an empty parameter and
# `just aws-up` writes a locally generated key into it, so the key exists in
# SSM and in the two peers' environments and nowhere else.
resource "aws_ssm_parameter" "session_key" {
  name        = "/${local.name}/session-key"
  description = "Ephemeral HMAC-SHA256 key for the current session. Written by just aws-up, never by Terraform."
  type        = "SecureString"
  value       = "placeholder-overwritten-by-just-aws-up"
  tier        = "Standard"

  # The whole point is that Terraform does not track the real value.
  lifecycle {
    ignore_changes = [value]
  }

  tags = { Name = "${local.name}-session-key" }
}

# --- instance identity -----------------------------------------------------

resource "aws_iam_role" "peer" {
  name = "${local.name}-peer"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Action    = "sts:AssumeRole"
      Principal = { Service = "ec2.amazonaws.com" }
    }]
  })

  tags = { Name = "${local.name}-peer" }
}

# SSM Session Manager, so administration needs no SSH and no inbound rule.
resource "aws_iam_role_policy_attachment" "ssm" {
  role       = aws_iam_role.peer.name
  policy_arn = "arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore"
}

# Least privilege on purpose: this bucket, this prefix, this parameter.
resource "aws_iam_role_policy" "artifacts" {
  name = "${local.name}-artifacts"
  role = aws_iam_role.peer.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid      = "ListOwnBucket"
        Effect   = "Allow"
        Action   = ["s3:ListBucket"]
        Resource = [aws_s3_bucket.artifacts.arn]
      },
      {
        Sid    = "ReadWriteOwnObjects"
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:DeleteObject",
        ]
        Resource = ["${aws_s3_bucket.artifacts.arn}/*"]
      },
      {
        Sid      = "ReadSessionKey"
        Effect   = "Allow"
        Action   = ["ssm:GetParameter"]
        Resource = [aws_ssm_parameter.session_key.arn]
      },
    ]
  })
}

resource "aws_iam_instance_profile" "peer" {
  name = "${local.name}-peer"
  role = aws_iam_role.peer.name
}

# --- the instance ----------------------------------------------------------

resource "aws_instance" "peer" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.instance_type
  subnet_id              = aws_subnet.public.id
  vpc_security_group_ids = [aws_security_group.peer.id]
  iam_instance_profile   = aws_iam_instance_profile.peer.name

  # No key pair. There is nothing to SSH into.
  key_name = null

  # A shutdown terminates rather than stops, so the timer in user-data is a
  # real deadline and not a way to accumulate stopped instances with volumes.
  instance_initiated_shutdown_behavior = "terminate"

  metadata_options {
    http_endpoint = "enabled"
    # IMDSv2 only: the token requirement is what stops a confused-deputy read of
    # the instance credentials through a request the application was tricked
    # into making.
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
  }

  root_block_device {
    volume_size           = var.volume_size_gb
    volume_type           = "gp3"
    encrypted             = true
    delete_on_termination = true
  }

  user_data_replace_on_change = true
  user_data = templatefile("${path.module}/user_data.sh.tftpl", {
    project             = local.name
    bucket              = aws_s3_bucket.artifacts.bucket
    region              = var.region
    session_key_param   = aws_ssm_parameter.session_key.name
    session_port        = var.session_port
    auto_shutdown_hours = var.auto_shutdown_hours
  })

  tags = { Name = "${local.name}-peer" }
}

# A stable address, so the client's `--peer` argument survives a rebuild.
resource "aws_eip" "peer" {
  instance = aws_instance.peer.id
  domain   = "vpc"

  tags = { Name = "${local.name}-peer" }

  depends_on = [aws_internet_gateway.lab]
}
