# Copy to terraform.tfvars and edit. terraform.tfvars is gitignored.
#
# Find your public address with:
#   curl -s https://checkip.amazonaws.com

# The only address allowed to reach UDP/7000. Keep it a /32.
allowed_cidr = "203.0.113.7/32"

# eu-central-1 (Frankfurt) is the experiment. Changing it changes the result.
region = "eu-central-1"

# Move to t3.medium if the smoke benchmark cannot hold 60 Hz.
instance_type = "t3.small"

# Backstop against a forgotten instance. Paired with shutdown-behaviour=terminate.
auto_shutdown_hours = 4
