variable "region" {
  description = "AWS region. Frankfurt is the point of the experiment: far enough from Brazil to make the rollback visible, close enough to stay playable."
  type        = string
  default     = "eu-central-1"
}

variable "project" {
  description = "Name prefix for every resource, so a stray object is always traceable to this lab."
  type        = string
  default     = "rollback-netcode"
}

variable "instance_type" {
  description = "EC2 instance type. Start at t3.small; move to t3.medium if the smoke benchmark cannot hold 60 Hz."
  type        = string
  default     = "t3.small"

  validation {
    condition     = contains(["t3.small", "t3.medium", "t3.large"], var.instance_type)
    error_message = "Use t3.small, t3.medium or t3.large; anything else is outside the lab's cost envelope."
  }
}

variable "allowed_cidr" {
  description = "The single public CIDR allowed to reach UDP/7000. Your home address, as a /32."
  type        = string

  validation {
    condition     = can(cidrhost(var.allowed_cidr, 0))
    error_message = "allowed_cidr must be valid CIDR notation, e.g. 203.0.113.7/32."
  }

  validation {
    condition     = var.allowed_cidr != "0.0.0.0/0"
    error_message = "Refusing to open the game port to the whole internet. Pass your own address as a /32."
  }
}

variable "session_port" {
  description = "UDP port the session runs on."
  type        = number
  default     = 7000
}

variable "volume_size_gb" {
  description = "Root volume size. The FBNeo core alone is ~90 MB; the rest is logs and build artefacts."
  type        = number
  default     = 20
}

variable "auto_shutdown_hours" {
  description = "Hours after boot at which the instance shuts itself down. Paired with instance_initiated_shutdown_behavior=terminate, this is the backstop against a forgotten instance."
  type        = number
  default     = 4

  validation {
    condition     = var.auto_shutdown_hours > 0 && var.auto_shutdown_hours <= 12
    error_message = "auto_shutdown_hours must be between 1 and 12; the lab is not meant to run overnight."
  }
}

variable "log_retention_days" {
  description = "Days after which S3 expires collected logs and artefacts."
  type        = number
  default     = 7
}
