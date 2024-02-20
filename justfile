set dotenv-load
task_arn := `aws --profile sub_account --region eu-central-1 ecs list-tasks --cluster ultrafinance-production | jq '.taskArns[0]' -r`
task_id := `aws --profile sub_account --region eu-central-1 ecs list-tasks --cluster ultrafinance-production | jq '.taskArns[0]' -r | awk -F/ '{print $NF}'`

terraform-apply:
	cd terraform && TF_VAR_ecr_image_revision=$(terraform output --raw deployed_revision) terraform apply -var-file=".tfvars" -auto-approve -lock=false -refresh=false
logs:
	aws --profile sub_account --region eu-central-1 logs tail ultrafinance/web --follow
import-db:
	mysqldump --single-transaction --set-gtid-purged=OFF -h {{env_var('PRODUCTION_DATABASE_HOST')}} -u {{env_var('PRODUCTION_DATABASE_USER')}} -p'{{env_var('PRODUCTION_DATABASE_PASSWORD')}}' {{env_var('PRODUCTION_DATABASE_NAME')}} | mysql -h 127.0.0.1 -u root ultrafinance
deploy-db:
	mysqldump --single-transaction --set-gtid-purged=OFF -h 127.0.0.1 -u root ultrafinance | mysql -h {{env_var('PRODUCTION_DATABASE_HOST')}} -u {{env_var('PRODUCTION_DATABASE_USER')}} -p'{{env_var('PRODUCTION_DATABASE_PASSWORD')}}' {{env_var('PRODUCTION_DATABASE_NAME')}}
deploy:
	./release.sh
ssh:
	aws --profile sub_account --region eu-central-1 ecs execute-command  \
		--cluster ultrafinance-production \
		--task {{task_id}} \
		--container web \
		--command "/bin/bash" \
		--interactive
