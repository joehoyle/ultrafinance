mock_provider "aws" {
  alias = "us_east_1"
}

mock_provider "aws" {
  mock_resource "aws_iam_role" {
    defaults = {
      arn = "arn:aws:iam::123456789012:role/ultrafinance-lambda"
    }
  }
  mock_resource "aws_lambda_function" {
    defaults = {
      arn     = "arn:aws:lambda:ca-central-1:123456789012:function:ultrafinance"
      version = "1"
    }
  }
}

run "repository_bootstrap" {
  command = plan
  variables {
    domain_name              = null
    image_uri                = null
    github_repository        = null
    github_oidc_provider_arn = null
  }
  assert {
    condition     = length(aws_lambda_function.app) == 0 && length(aws_iam_role.deploy) == 0
    error_message = "Default bootstrap must not create an application or an unconfigured CI role."
  }
}

run "alias_and_oidc" {
  command = plan
  variables {
    domain_name          = null
    image_uri            = "123456789012.dkr.ecr.ca-central-1.amazonaws.com/ultrafinance@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    github_repository    = "joehoyle/ultrafinance"
    github_owner_id      = "161683"
    github_repository_id = "1410780058"
  }
  assert {
    condition     = aws_lambda_function_url.app[0].qualifier == "live" && aws_lambda_function_url.app[0].authorization_type == "NONE"
    error_message = "The public endpoint must target the stable live alias."
  }
  assert {
    condition     = aws_lambda_permission.function_url[0].qualifier == "live" && aws_lambda_permission.function_url_invoke[0].qualifier == "live" && aws_lambda_permission.function_url_invoke[0].invoked_via_function_url
    error_message = "Public invocation permissions must be limited to the live function URL."
  }
  assert {
    condition     = toset(jsondecode(aws_iam_role.deploy[0].assume_role_policy).Statement[0].Condition.StringEquals["token.actions.githubusercontent.com:sub"]) == toset(["repo:joehoyle/ultrafinance:ref:refs/heads/main", "repo:joehoyle@161683/ultrafinance@1410780058:ref:refs/heads/main"])
    error_message = "OIDC must trust only this repository's main branch, with exact subjects."
  }
}
