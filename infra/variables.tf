variable "aws_profile" {
  type    = string
  default = "joehoyle"
}
variable "domain_name" {
  description = "Optional public domain whose existing Route 53 zone must be imported before applying."
  type        = string
  default     = null
}
variable "aws_use_cli_credentials" {
  description = "Use temporary credentials exported by infra/tofu.sh for AWS CLI login profiles."
  type        = bool
  default     = false
}
variable "aws_account_id" {
  description = "Optional AWS account guard for infrastructure operations."
  type        = string
  default     = null
  validation {
    condition     = var.aws_account_id == null ? true : can(regex("^[0-9]{12}$", var.aws_account_id))
    error_message = "AWS account ID must contain 12 digits."
  }
}
variable "aws_region" {
  type    = string
  default = "ca-central-1"
}
variable "name" {
  type    = string
  default = "ultrafinance"
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,39}$", var.name))
    error_message = "Use a lowercase name with 2-40 letters, numbers, or hyphens."
  }
}
variable "image_uri" {
  description = "ECR image digest URI. Leave null for the initial repository-only bootstrap."
  type        = string
  default     = null
  validation {
    condition     = var.image_uri == null ? true : can(regex("^[0-9]{12}\\.dkr\\.ecr\\.[a-z0-9-]+\\.amazonaws\\.com/[a-z0-9/_-]+@sha256:[a-f0-9]{64}$", var.image_uri))
    error_message = "Use an immutable ECR digest URI, not a mutable image tag."
  }
}
variable "typesafe_api_key" {
  type      = string
  sensitive = true
  default   = ""
}
variable "jev_model" {
  type    = string
  default = "jev-latest"
}
variable "match_threshold" {
  type    = number
  default = 0.95
  validation {
    condition     = var.match_threshold >= 0 && var.match_threshold <= 1
    error_message = "Match threshold must be between 0 and 1."
  }
}

variable "github_repository" {
  description = "Exact owner/repo allowed to deploy from main. Null disables CI resources."
  type        = string
  default     = null
  validation {
    condition     = var.github_repository == null ? true : can(regex("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", var.github_repository))
    error_message = "Use an exact GitHub owner/repository name."
  }
}
variable "github_owner_id" {
  description = "Numeric owner ID for GitHub immutable OIDC subjects. Set together with github_repository_id."
  type        = string
  default     = null
  validation {
    condition     = var.github_owner_id == null ? true : can(regex("^[0-9]+$", var.github_owner_id))
    error_message = "GitHub owner ID must be numeric."
  }
}
variable "github_repository_id" {
  description = "Numeric repository ID for GitHub immutable OIDC subjects."
  type        = string
  default     = null
  validation {
    condition     = var.github_repository_id == null ? true : can(regex("^[0-9]+$", var.github_repository_id))
    error_message = "GitHub repository ID must be numeric."
  }
}
variable "github_oidc_provider_arn" {
  description = "Reuse an existing account-wide GitHub OIDC provider ARN, or leave null to create it."
  type        = string
  default     = null
}
