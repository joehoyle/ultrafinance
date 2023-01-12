use crate::models::Function;
use deno_runtime::deno_core::v8;
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub struct FunctionRuntime {
    runtime: deno_runtime::worker::MainWorker,
    main_module_id: i32,
}

impl FunctionRuntime {
    pub fn new(function: &Function) -> anyhow::Result<Self> {
        let mut module_loader = ModuleLoader::new();
        let mut main_file = "index.ts";

        if function.function_type == "built-in" {
            main_file = &function.source;
            module_loader.files.insert(
                main_file.to_owned(),
                include_str!("functions/lunchmoney.ts").to_owned(),
            );
        } else {
            module_loader.files.insert(
                main_file.to_owned(),
                function.source.clone(),
            );
        }

        let module_loader = std::rc::Rc::new(module_loader);
        let create_web_worker_cb = std::sync::Arc::new(|_| {
            todo!("Web workers are not supported in the example");
        });
        let web_worker_event_cb = std::sync::Arc::new(|_| {
            todo!("Web workers are not supported in the example");
        });

        let options = deno_runtime::worker::WorkerOptions {
            bootstrap: deno_runtime::BootstrapOptions {
                args: vec![],
                cpu_count: 1,
                debug_flag: false,
                enable_testing_features: false,
                location: None,
                no_color: false,
                is_tty: false,
                runtime_version: "1.0.0".to_string(),
                ts_version: "x".to_string(),
                unstable: false,
                user_agent: "ultrafinance".to_string(),
                inspect: false,
            },
            extensions: vec![],
            unsafely_ignore_certificate_errors: None,
            root_cert_store: None,
            seed: None,
            source_map_getter: None,
            format_js_error_fn: None,
            web_worker_preload_module_cb: web_worker_event_cb.clone(),
            web_worker_pre_execute_module_cb: web_worker_event_cb,
            create_web_worker_cb,
            maybe_inspector_server: None,
            should_break_on_first_statement: false,
            module_loader,
            npm_resolver: None,
            get_error_class_fn: None,
            cache_storage_dir: None,
            origin_storage_dir: None,
            blob_store: deno_runtime::deno_web::BlobStore::default(),
            broadcast_channel:
                deno_runtime::deno_broadcast_channel::InMemoryBroadcastChannel::default(),
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            stdio: Default::default(),
        };

        let js_path = std::path::Path::new(&main_file);
        let main_module = deno_runtime::deno_core::resolve_path(&js_path.to_string_lossy())?;
        let permissions = deno_runtime::permissions::Permissions::allow_all();

        let mut worker = deno_runtime::worker::MainWorker::bootstrap_from_options(
            main_module.clone(),
            permissions,
            options,
        );
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        let local = tokio::task::LocalSet::new();
        let result: anyhow::Result<i32> = local.block_on(&mut rt, async {
            let module_id = worker.preload_main_module(&main_module).await?;
            worker.evaluate_module(module_id).await?;
            worker.run_event_loop(false).await?;
            Ok(module_id)
        });
        let result = result?;
        Ok(Self {
            runtime: worker,
            main_module_id: result,
        })
    }

    pub fn get_params(&mut self) -> anyhow::Result<FunctionParams> {
        let module_namespace = self
            .runtime
            .js_runtime
            .get_module_namespace(self.main_module_id)?;
        let scope = &mut self.runtime.js_runtime.handle_scope();
        let module_namespace = v8::Local::<v8::Object>::new(scope, module_namespace);
        let export_name = v8::String::new(scope, "params").unwrap();
        let binding = module_namespace.get(scope, export_name.into());
        match binding {
            Some(value) => {
                if !value.is_object() {
                    return Err(anyhow::anyhow!("No params export found."));
                }
                let params: FunctionParams =
                    deno_runtime::deno_core::serde_v8::from_v8(scope, value)?;
                Ok(params)
            }
            None => Err(anyhow::Error::msg("No params export found.")),
        }
    }

    pub fn run(&mut self, params: &String, payload: &String) -> anyhow::Result<String> {
        let mut rt = tokio::runtime::Runtime::new().unwrap();
        let local = tokio::task::LocalSet::new();
        let result = local.block_on(&mut rt, async {
            let promise: v8::Global<v8::Value>;
            {
                let module_namespace = self
                    .runtime
                    .js_runtime
                    .get_module_namespace(self.main_module_id)?;
                let scope = &mut self.runtime.js_runtime.handle_scope();
                let module_namespace = v8::Local::<v8::Object>::new(scope, module_namespace);
                let export_name = v8::String::new(scope, "default").unwrap();
                let binding = module_namespace.get(scope, export_name.into());
                promise = match binding {
                    Some(value) => {
                        if !value.is_function() {
                            return Err(anyhow::anyhow!("No default export function found."));
                        }
                        let function: v8::Local<v8::Function> = value.try_into()?;
                        let recv = v8::undefined(scope).into();

                        // JSON decode params
                        let json = deno_runtime::deno_core::JsRuntime::grab_global::<v8::Object>(
                            scope, "JSON",
                        )
                        .unwrap();
                        let json = v8::Global::new(scope, json);

                        let parse = v8::String::new(scope, "parse").unwrap().into();
                        let parse = json.open(scope).get(scope, parse).unwrap();
                        let parse: v8::Local<v8::Function> = parse.try_into()?;

                        let arg = v8::String::new(scope, params.as_str()).unwrap().into();
                        let this = v8::undefined(scope).into();
                        let params = match parse.call(scope, this, &[arg]) {
                            Some(r) => Ok(r),
                            None => Err(anyhow::anyhow!("Error decoding params.")),
                        }?;

                        let arg = v8::String::new(scope, payload.as_str()).unwrap().into();
                        let this = v8::undefined(scope).into();
                        let payload = match parse.call(scope, this, &[arg]) {
                            Some(r) => Ok(r),
                            None => Err(anyhow::anyhow!("Error decoding payload.")),
                        }?;

                        let try_scope = &mut v8::TryCatch::new(scope);
                        let value = function.call(try_scope, recv, &[params, payload]).unwrap();
                        if try_scope.has_caught() || try_scope.has_terminated() {
                            dbg!("caught terminated");
                            try_scope.rethrow();
                            return Ok("".to_owned());
                        };

                        let promise_global = v8::Global::new(try_scope, value);
                        Ok(promise_global)
                    }
                    None => Err(anyhow::Error::msg("No default export found.")),
                }?;
            }

            self.runtime.js_runtime.run_event_loop(false).await?;
            let result = self.runtime.js_runtime.resolve_value(promise).await?;

            let scope = &mut self.runtime.js_runtime.handle_scope();
            let json = deno_runtime::deno_core::JsRuntime::grab_global::<v8::Object>(scope, "JSON")
                .unwrap();
            let json = v8::Global::new(scope, json);

            let stringify = v8::String::new(scope, "stringify").unwrap().into();
            let stringify = json.open(scope).get(scope, stringify).unwrap();
            let stringify: v8::Local<v8::Function> = stringify.try_into()?;
            let result = v8::Local::<v8::Value>::new(scope, &result);
            let this = v8::undefined(scope).into();
            let result = match stringify.call(scope, this, &[result]) {
                Some(r) => Ok(r),
                None => Err(anyhow::anyhow!("Error stringifying result.")),
            }?;

            Ok(result.to_rust_string_lossy(scope))
        })?;
        return Ok(result);
    }
}

#[derive(Deserialize, Debug, Serialize, Apiv2Schema, ts_rs::TS)]
#[allow(dead_code)]
pub struct FunctionParam {
    name: String,
    r#type: String,
}

pub type FunctionParams = HashMap<String, FunctionParam>;

pub struct ModuleLoader {
    pub files: HashMap<String, String>,
}

impl ModuleLoader {
    pub fn new() -> Self {
        return Self {
            files: HashMap::new(),
        };
    }
}

impl deno_runtime::deno_core::ModuleLoader for ModuleLoader {
    fn resolve(
        &self,
        _specifier: &str,
        _referrer: &str,
        _is_main: bool,
    ) -> Result<deno_runtime::deno_core::ModuleSpecifier, anyhow::Error> {
        Ok(deno_runtime::deno_core::ModuleSpecifier::parse(_specifier)?)
    }

    fn load(
        &self,
        module_specifier: &deno_runtime::deno_core::ModuleSpecifier,
        _maybe_referrer: Option<deno_runtime::deno_core::ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> std::pin::Pin<Box<deno_runtime::deno_core::ModuleSourceFuture>> {
        use deno_ast::MediaType;
        use deno_runtime::deno_core::*;
        use futures::future::FutureExt;

        let module_specifier = module_specifier.clone();
        let files = self.files.clone();
        // let media_type = MediaType::from(&path);
        async move {
            let module_specifier = module_specifier.clone();
            let path = module_specifier
                .to_file_path()
                .map_err(|_| anyhow::anyhow!("Only file: URLs are supported."))?;
            let module_file = module_specifier.path().rsplitn(2, '/').next();
            let media_type = MediaType::from(&path);

            let code = match module_file {
                Some(filename) => files.get(filename),
                None => None,
            }
            .ok_or(anyhow::anyhow!("File not found."))?;

            let (module_type, should_transpile) = match MediaType::from(&path) {
                MediaType::JavaScript | MediaType::Mjs | MediaType::Cjs => {
                    (ModuleType::JavaScript, false)
                }
                MediaType::Jsx => (ModuleType::JavaScript, true),
                MediaType::TypeScript
                | MediaType::Mts
                | MediaType::Cts
                | MediaType::Dts
                | MediaType::Dmts
                | MediaType::Dcts
                | MediaType::Tsx => (ModuleType::JavaScript, true),
                MediaType::Json => (ModuleType::Json, false),
                _ => anyhow::bail!("Unknown extension {:?}", path.extension()),
            };

            let code = if should_transpile {
                let parsed = deno_ast::parse_module(deno_ast::ParseParams {
                    specifier: module_specifier.to_string(),
                    text_info: deno_ast::SourceTextInfo::from_string(code.clone()),
                    media_type,
                    capture_tokens: false,
                    scope_analysis: false,
                    maybe_syntax: None,
                })?;
                parsed.transpile(&Default::default())?.text
            } else {
                code.clone()
            };
            let module = ModuleSource {
                code: code.into_bytes().into_boxed_slice(),
                module_type,
                module_url_specified: module_specifier.to_string(),
                module_url_found: module_specifier.to_string(),
            };

            Ok(module)
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;

    #[test]
    fn get_params() {
        let function = Function {
            id: 1,
            name: "Test".into(),
            function_type: "built-in".into(),
            source: "lunchmoney.ts".into(),
            user_id: 1,
            created_at: NaiveDateTime::from_timestamp(0, 0),
            updated_at: NaiveDateTime::from_timestamp(0, 0),
        };

        let mut runtime = FunctionRuntime::new(&function).unwrap();
        dbg!(runtime.get_params().unwrap());
    }

    #[test]
    fn run_function() {
        let function = Function {
            id: 1,
            name: "Test".into(),
            function_type: "built-in".into(),
            source: "lunchmoney.ts".into(),
            user_id: 1,
            created_at: NaiveDateTime::from_timestamp(0, 0),
            updated_at: NaiveDateTime::from_timestamp(0, 0),
        };

        let mut runtime = FunctionRuntime::new(&function).unwrap();
        let params = serde_json::json!({
            "apiKey": "123",
            "accountId": "41112",
        })
        .to_string();
        let transaction = serde_json::json!({
            "id": 12345,
            "bookingDate": chrono::Utc::now().to_rfc3339(),
            "transactionAmount": "100",
            "transactionAmountCurrency": "EUR",
            "creditorName": "Joe Test",
            "remittanceInformation": "Test transaction",
        })
        .to_string();
        dbg!(&transaction);
        let _result = runtime.run(&params, &transaction);
    }
}
