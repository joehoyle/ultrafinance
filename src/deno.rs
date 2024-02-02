use crate::models::Function;
use anyhow::anyhow;
use deno_runtime::deno_core::OpState;
use deno_runtime::{
    deno_core::{op2, v8, ModuleSpecifier, Op, PollEventLoopOptions, ResolutionKind},
    permissions::PermissionsContainer,
};
use paperclip::actix::Apiv2Schema;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

pub struct FunctionRuntime {
    runtime: deno_runtime::worker::MainWorker,
    main_module_id: usize,
    stdio_rx: tokio::sync::mpsc::UnboundedReceiver<StdioMsg>,
}

#[derive(Debug, Serialize, ts_rs::TS, Apiv2Schema, Deserialize)]
#[ts(export)]
pub struct StdioMsg {
    pub msg: String,
    pub is_err: bool,
}

deno_core::extension!(ultrafinance,
  options = {
      sender: tokio::sync::mpsc::UnboundedSender<StdioMsg>,
  },
  middleware = |op| match op.name {
    "op_print" => <op_print as Op>::DECL,
    _ => op,
  },
  state = |state, options| {
      state.put(options.sender);
  },
);

#[op2(fast)]
pub fn op_print(
    state: &mut OpState,
    #[string] msg: &str,
    is_err: bool,
) -> Result<(), deno_core::error::AnyError> {
    let sender = state.borrow_mut::<tokio::sync::mpsc::UnboundedSender<StdioMsg>>();
    Ok(sender.send(StdioMsg {
        msg: msg.to_owned(),
        is_err,
    })?)
}

impl FunctionRuntime {
    pub async fn new(function: &Function) -> anyhow::Result<Self> {
        let mut module_loader = ModuleLoader::new();
        let mut main_file = "index.ts";

        if function.function_type == "built-in" {
            main_file = &function.source;
            module_loader.files.insert(
                main_file.to_owned(),
                include_str!("functions/lunchmoney.ts").to_owned(),
            );
        } else {
            module_loader
                .files
                .insert(main_file.to_owned(), function.source.clone());
        }
        let module_loader = std::rc::Rc::new(module_loader);
        let create_web_worker_cb = std::sync::Arc::new(|_| {
            todo!("Web workers are not supported in the example");
        });

        let (stdio_tx, stdio_rx) = tokio::sync::mpsc::unbounded_channel();

        let options = deno_runtime::worker::WorkerOptions {
            bootstrap: deno_runtime::BootstrapOptions {
                args: vec![],
                cpu_count: Default::default(),

                enable_testing_features: false,
                location: None,
                no_color: false,
                is_tty: false,
                // runtime_version: "1.0.0".to_string(),
                // ts_version: "x".to_string(),
                unstable: false,
                user_agent: "ultrafinance".to_string(),
                inspect: false,
                log_level: deno_runtime::WorkerLogLevel::Error,
                locale: "en-US".to_string(),
                has_node_modules_dir: false,
                maybe_binary_npm_command_name: None,
                enable_op_summary_metrics: true,
                unstable_features: vec![],
                node_ipc_fd: None,
            },
            extensions: vec![ultrafinance::init_ops(stdio_tx.clone())],
            unsafely_ignore_certificate_errors: None,
            seed: None,
            source_map_getter: None,
            format_js_error_fn: None,
            create_web_worker_cb,
            maybe_inspector_server: None,
            should_break_on_first_statement: false,
            module_loader,
            npm_resolver: None,
            get_error_class_fn: None,
            cache_storage_dir: None,
            origin_storage_dir: None,
            blob_store: Arc::new(deno_runtime::deno_web::BlobStore::default()),
            broadcast_channel:
                deno_runtime::deno_broadcast_channel::InMemoryBroadcastChannel::default(),
            shared_array_buffer_store: None,
            compiled_wasm_module_store: None,
            stdio: Default::default(),
            startup_snapshot: None,
            create_params: None,
            root_cert_store_provider: None,
            fs: Arc::new(deno_runtime::deno_fs::RealFs {}),
            should_wait_for_inspector_session: false,
            skip_op_registration: false,
            strace_ops: None,
            feature_checker: Default::default(),
        };

        let js_path = std::path::Path::new(&main_file);
        let main_module = ModuleSpecifier::parse(
            &format!("https://localhost/{}", &js_path.to_string_lossy()).as_str(),
        )?;
        let mut permissions = deno_runtime::permissions::Permissions::default();
        permissions.net =
            deno_runtime::permissions::Permissions::new_net(&Some(vec![]), &None, false)?;

        let mut worker = deno_runtime::worker::MainWorker::bootstrap_from_options(
            main_module.clone(),
            PermissionsContainer::new(permissions),
            options,
        );

        let result: anyhow::Result<i32> = {
            let module_id = worker.preload_main_module(&main_module).await?;
            worker.evaluate_module(module_id).await?;
            worker.run_event_loop(false).await?;
            Ok(module_id as _)
        };
        let result = result?;
        Ok(Self {
            runtime: worker,
            main_module_id: result as _,
            stdio_rx,
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

    pub async fn run(&mut self, params: &str, payload: &str) -> anyhow::Result<String> {
        // let mut rt = tokio::runtime::Runtime::new().unwrap();
        // let local = tokio::task::LocalSet::new();
        // let handle = tokio::runtime::Handle::current();
        let promise: Result<v8::Global<v8::Value>, anyhow::Error>;
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
                    let json =
                        deno_runtime::deno_core::JsRuntime::eval::<v8::Object>(scope, "JSON")
                            .ok_or(anyhow::anyhow!("JSON not found."))?;
                    let json = v8::Global::new(scope, json);

                    let parse = v8::String::new(scope, "parse")
                        .ok_or(anyhow!("Can't make string"))?
                        .into();
                    let parse = json
                        .open(scope)
                        .get(scope, parse)
                        .ok_or(anyhow!("Can't get string"))?;
                    let parse: v8::Local<v8::Function> = parse.try_into()?;

                    let arg = v8::String::new(scope, params)
                        .ok_or(anyhow!("Can't make string"))?
                        .into();
                    let this = v8::undefined(scope).into();
                    let params = match parse.call(scope, this, &[arg]) {
                        Some(r) => Ok(r),
                        None => Err(anyhow::anyhow!("Error decoding params.")),
                    }?;

                    let arg = v8::String::new(scope, payload).unwrap().into();
                    let this = v8::undefined(scope).into();
                    let payload = match parse.call(scope, this, &[arg]) {
                        Some(r) => Ok(r),
                        None => Err(anyhow::anyhow!("Error decoding payload.")),
                    }?;

                    let try_scope = &mut v8::TryCatch::new(scope);
                    let value = function
                        .call(try_scope, recv, &[params, payload])
                        .ok_or(anyhow!("No globa promise found"))?;
                    if try_scope.has_caught() || try_scope.has_terminated() {
                        dbg!("caught terminated");
                        try_scope.rethrow();
                        return Ok("".to_owned());
                    };

                    let promise_global = v8::Global::new(try_scope, value);
                    Ok(promise_global)
                }
                None => Err(anyhow::Error::msg("No default export found.")),
            }
        }
        let promise = promise?;
        self.runtime
            .js_runtime
            .run_event_loop(PollEventLoopOptions::default())
            .await?;

        #[allow(deprecated)]
        let result = self.runtime.js_runtime.resolve_value(promise).await?;

        let scope = &mut self.runtime.js_runtime.handle_scope();
        let json = deno_runtime::deno_core::JsRuntime::eval::<v8::Object>(scope, "JSON").unwrap();
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
    }

    pub async fn get_output(&mut self) -> Vec<StdioMsg> {
        let mut output = Vec::new();
        while let Ok(msg) = self.stdio_rx.try_recv() {
            output.push(msg);
        }
        output
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
        _kind: ResolutionKind,
    ) -> Result<deno_runtime::deno_core::ModuleSpecifier, anyhow::Error> {
        let url = match _referrer {
            "." => deno_runtime::deno_core::ModuleSpecifier::parse(_specifier)?,
            _ => {
                let referrer = deno_runtime::deno_core::ModuleSpecifier::parse(_referrer)?;
                let referrer_path = referrer.join(_specifier)?;
                referrer_path
            }
        };
        Ok(url)
    }

    fn load(
        &self,
        module_specifier: &deno_runtime::deno_core::ModuleSpecifier,
        _maybe_referrer: Option<&deno_runtime::deno_core::ModuleSpecifier>,
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

            let file_path = module_specifier.to_file_path();
            let result: anyhow::Result<(MediaType, String)> = match file_path {
                Ok(path) => {
                    let module_file = module_specifier.path().rsplitn(2, '/').next();
                    let media_type = MediaType::from_path(&path);

                    let code = match module_file {
                        Some(filename) => files.get(filename),
                        None => None,
                    }
                    .ok_or(anyhow::anyhow!("File not found."))?;
                    Ok((media_type, code.to_owned()))
                }
                Err(_) => {
                    let code = reqwest::get(module_specifier.clone()).await?;
                    let content_type = code
                        .headers()
                        .get("content-type")
                        .ok_or(anyhow::anyhow!("No content type header found."))?;
                    let media_type = MediaType::from_specifier_and_content_type(
                        &module_specifier,
                        content_type.to_str().ok(),
                    );
                    let code = code.text().await?;
                    Ok((media_type, code))
                }
            };

            let (media_type, code) = result?;

            let (module_type, should_transpile) = match media_type {
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
                _ => anyhow::bail!("Unknown type {}", media_type),
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

            let code = FastString::Owned(code.into_boxed_str());
            let module = ModuleSource::new(module_type, code, &module_specifier);

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

        let rt = tokio::runtime::Runtime::new().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            FunctionRuntime::new(&function).await.unwrap()
        });
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

        let rt = tokio::runtime::Runtime::new().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let mut runtime = FunctionRuntime::new(&function).await.unwrap();
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
            let _result = runtime.run(&params, &transaction).await;
        });
    }

    #[test]
    fn console_log() {
        let function = Function {
            id: 1,
            name: "Test".into(),
            function_type: "user".into(),
            source: "
            console.log(new Date());
            console.log('Hello world');
            console.log(true);
            console.log(false);
            console.log(1);
            "
            .into(),
            user_id: 1,
            created_at: NaiveDateTime::from_timestamp(0, 0),
            updated_at: NaiveDateTime::from_timestamp(0, 0),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let mut runtime = FunctionRuntime::new(&function).await.unwrap();
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
            dbg!(runtime.get_output().await);
        });
    }

    #[test]
    fn run_source_function() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let function = Function {
                id: 1,
                name: "Test".into(),
                function_type: "source".into(),
                source: "export default function () {
                    return 'ji';
                }"
                .into(),
                user_id: 1,
                created_at: NaiveDateTime::from_timestamp(0, 0),
                updated_at: NaiveDateTime::from_timestamp(0, 0),
            };

            let mut runtime = FunctionRuntime::new(&function).await.unwrap();
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
            runtime.run(&params, &transaction).await.unwrap();
        });
    }

    #[test]
    fn external_dep() {
        let function = Function {
            id: 1,
            name: "Test".into(),
            function_type: "user".into(),
            source: r#"
            import { camelCase } from "https://deno.land/x/camelcase/mod.ts";
            export default async function() {
                console.log(camelCase)
                return camelCase("hello world");
            }
            "#
            .into(),
            user_id: 1,
            created_at: NaiveDateTime::from_timestamp(0, 0),
            updated_at: NaiveDateTime::from_timestamp(0, 0),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async {
            let mut runtime = FunctionRuntime::new(&function).await.unwrap();
            let result = runtime.run("{}", "{}").await.unwrap();
            assert_eq!(r#""helloWorld""#, result);
            dbg!(runtime.get_output().await);
        });
    }
}
