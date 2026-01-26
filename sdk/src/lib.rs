use futures_util::FutureExt;
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::HashMap, panic::AssertUnwindSafe};
use std::{future::Future, marker::PhantomData};
use tokio::net::UnixListener;
use tonic::Response;

use tonic::Status;

mod action {
    tonic::include_proto!("noctiforge.action");
}

use action::{
    InvokeRequest, InvokeResult, Success,
    function_runner_service_server::{FunctionRunnerService, FunctionRunnerServiceServer},
    invoke_result::Result as IR,
};

pub use action::Problem;

#[allow(dead_code)]
pub struct Context {
    pub values: HashMap<String, String>,
}

pub(crate) struct MyService<F, Fut, In, Out>
where
    F: Send + Sync + Clone + 'static + Fn(In, Context) -> Fut,
    Fut: Future<Output = Result<Out, Problem>> + Send + 'static,
    In: DeserializeOwned + Send + 'static,
    Out: Serialize + Send + Sync + 'static,
{
    handler: F,
    _marker: PhantomData<(Fut, In, Out)>,
}

#[tonic::async_trait]
impl<F, Fut, In, Out> FunctionRunnerService for MyService<F, Fut, In, Out>
where
    F: Send + Sync + Clone + 'static + Fn(In, Context) -> Fut,
    Fut: Future<Output = Result<Out, Problem>> + Send + 'static + std::marker::Sync,
    In: DeserializeOwned + Send + 'static + std::marker::Sync,
    Out: Serialize + Send + Sync + 'static,
{
    async fn invoke(
        &self,
        request: tonic::Request<InvokeRequest>,
    ) -> Result<Response<InvokeResult>, Status> {
        let inner = request.into_inner();

        let input: In = serde_json::from_slice(&inner.payload)
            .map_err(|e| Status::invalid_argument(format!("Invalid input: {}", e)))?;

        let context = Context {
            values: inner.metadata,
        };

        let fut = {
            let handler = self.handler.clone();
            handler(input, context)
        };

        let result = AssertUnwindSafe(fut)
            .catch_unwind()
            .await
            .map_err(|panic| Status::internal(format!("{:?}", panic)))?;

        let invoke_result = match result {
            Ok(out) => {
                let bytes = serde_json::to_vec(&out)
                    .map_err(|_| Status::data_loss("Could not convert output to json"))?;

                IR::Success(Success { output: bytes })
            }
            Err(problem) => IR::Problem(problem),
        };

        Ok(Response::new(InvokeResult {
            result: Some(invoke_result),
        }))
    }
}

pub async fn start<F, Fut, In, Out>(handler: F) -> Result<(), Box<dyn std::error::Error>>
where
    F: Send + Sync + Clone + 'static + Fn(In, Context) -> Fut,
    Fut: Future<Output = Result<Out, Problem>> + Send + Sync + 'static,
    In: DeserializeOwned + Send + Sync + 'static,
    Out: Serialize + Send + Sync + 'static,
{
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let socket_path = "/run/app.sock";

    if std::path::Path::new(&socket_path).exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    log::info!("Starting server on Unix socket: {}", socket_path);

    let svc = FunctionRunnerServiceServer::new(MyService {
        handler,
        _marker: PhantomData,
    });

    tonic::transport::Server::builder()
        .add_service(svc)
        .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
        .await?;

    Ok(())
}

#[cfg(test)]
mod lib_tests {
    use super::*;
    use hyper_util::rt::TokioIo;
    use serde::{Deserialize, Serialize};
    use std::time::Duration;
    use tokio::time::timeout;
    use tonic::transport::{Channel, Endpoint, Uri};
    use tower::service_fn;

    use action::{
        InvokeRequest, function_runner_service_client::FunctionRunnerServiceClient,
        invoke_result::Result as IR,
    };

    // Test data structures
    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestInput {
        value: i32,
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestOutput {
        result: i32,
    }

    // Helper to create a test client connected to a Unix socket
    async fn create_test_client(
        socket_path: &str,
    ) -> Result<FunctionRunnerServiceClient<Channel>, Box<dyn std::error::Error>> {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let socket_path = socket_path.to_string();

        let channel = Endpoint::try_from("http://[::]:50051")?
            .connect_with_connector(service_fn(move |_: Uri| {
                let path = socket_path.to_string();
                async move {
                    let stream = tokio::net::UnixStream::connect(path).await?;
                    Ok::<_, std::io::Error>(TokioIo::new(stream))
                }
            }))
            .await?;

        Ok(FunctionRunnerServiceClient::new(channel))
    }

    #[tokio::test]
    async fn test_successful_invocation() {
        let socket_path = "/tmp/test_success.sock";

        // Clean up any existing socket
        let _ = std::fs::remove_file(socket_path);

        // Handler that doubles the input
        let handler = |input: TestInput, _ctx: Context| async move {
            Ok(TestOutput {
                result: input.value * 2,
            })
        };

        // Start server in background
        let server_handle = {
            let socket = socket_path.to_string();
            tokio::spawn(async move {
                let listener = UnixListener::bind(&socket).unwrap();
                let svc = action::function_runner_service_server::FunctionRunnerServiceServer::new(
                    crate::MyService {
                        handler,
                        _marker: PhantomData,
                    },
                );
                tonic::transport::Server::builder()
                    .add_service(svc)
                    .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
                    .await
                    .unwrap();
            })
        };

        // Create client and test
        let mut client = create_test_client(socket_path).await.unwrap();

        let request = InvokeRequest {
            payload: serde_json::to_vec(&TestInput { value: 21 }).unwrap(),
            metadata: HashMap::new(),
        };

        let response = timeout(Duration::from_secs(2), client.invoke(request))
            .await
            .unwrap()
            .unwrap();

        let result = response.into_inner();
        assert!(result.result.is_some());

        match result.result.unwrap() {
            IR::Success(success) => {
                let output: TestOutput = serde_json::from_slice(&success.output).unwrap();
                assert_eq!(output.result, 42);
            }
            IR::Problem(_) => panic!("Expected success, got problem"),
        }

        server_handle.abort();
        let _ = std::fs::remove_file(socket_path);
    }

    #[tokio::test]
    async fn test_handler_returns_problem() {
        let socket_path = "/tmp/test_problem.sock";
        let _ = std::fs::remove_file(socket_path);

        // Handler that returns a Problem for negative values
        let handler = |input: TestInput, _ctx: Context| async move {
            if input.value < 0 {
                Err(Problem {
                    r#type: "validation_error".to_string(),
                    detail: "Negative values not allowed".to_string(),
                })
            } else {
                Ok(TestOutput {
                    result: input.value,
                })
            }
        };

        let server_handle = {
            let socket = socket_path.to_string();
            tokio::spawn(async move {
                let listener = UnixListener::bind(&socket).unwrap();
                let svc = action::function_runner_service_server::FunctionRunnerServiceServer::new(
                    crate::MyService {
                        handler,
                        _marker: PhantomData,
                    },
                );
                tonic::transport::Server::builder()
                    .add_service(svc)
                    .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
                    .await
                    .unwrap();
            })
        };

        let mut client = create_test_client(socket_path).await.unwrap();

        let request = InvokeRequest {
            payload: serde_json::to_vec(&TestInput { value: -5 }).unwrap(),
            metadata: HashMap::new(),
        };

        let response = client.invoke(request).await.unwrap();
        let result = response.into_inner();

        match result.result.unwrap() {
            IR::Problem(problem) => {
                assert_eq!(problem.r#type, "validation_error");
                assert_eq!(problem.detail, "Negative values not allowed");
            }
            IR::Success(_) => panic!("Expected problem, got success"),
        }

        server_handle.abort();
        let _ = std::fs::remove_file(socket_path);
    }

    #[tokio::test]
    async fn test_invalid_input_payload() {
        let socket_path = "/tmp/test_invalid_input.sock";
        let _ = std::fs::remove_file(socket_path);

        let handler = |input: TestInput, _ctx: Context| async move {
            Ok(TestOutput {
                result: input.value,
            })
        };

        let server_handle = {
            let socket = socket_path.to_string();
            tokio::spawn(async move {
                let listener = UnixListener::bind(&socket).unwrap();
                let svc = action::function_runner_service_server::FunctionRunnerServiceServer::new(
                    crate::MyService {
                        handler,
                        _marker: PhantomData,
                    },
                );
                tonic::transport::Server::builder()
                    .add_service(svc)
                    .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
                    .await
                    .unwrap();
            })
        };

        let mut client = create_test_client(socket_path).await.unwrap();

        // Send invalid JSON
        let request = InvokeRequest {
            payload: b"not valid json".to_vec(),
            metadata: HashMap::new(),
        };

        let response = client.invoke(request).await;
        assert!(response.is_err());

        let err = response.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        assert!(err.message().contains("Invalid input"));

        server_handle.abort();
        let _ = std::fs::remove_file(socket_path);
    }

    #[tokio::test]
    async fn test_context_metadata() {
        let socket_path = "/tmp/test_context.sock";
        let _ = std::fs::remove_file(socket_path);

        #[derive(Serialize, Deserialize)]
        struct MetadataOutput {
            user_id: Option<String>,
        }

        let handler = |_input: TestInput, ctx: Context| async move {
            Ok(MetadataOutput {
                user_id: ctx.values.get("user_id").cloned(),
            })
        };

        let server_handle = {
            let socket = socket_path.to_string();
            tokio::spawn(async move {
                let listener = UnixListener::bind(&socket).unwrap();
                let svc = action::function_runner_service_server::FunctionRunnerServiceServer::new(
                    crate::MyService {
                        handler,
                        _marker: PhantomData,
                    },
                );
                tonic::transport::Server::builder()
                    .add_service(svc)
                    .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
                    .await
                    .unwrap();
            })
        };

        let mut client = create_test_client(socket_path).await.unwrap();

        let mut metadata = HashMap::new();
        metadata.insert("user_id".to_string(), "user123".to_string());

        let request = InvokeRequest {
            payload: serde_json::to_vec(&TestInput { value: 1 }).unwrap(),
            metadata,
        };

        let response = client.invoke(request).await.unwrap();
        let result = response.into_inner();

        match result.result.unwrap() {
            IR::Success(success) => {
                let output: MetadataOutput = serde_json::from_slice(&success.output).unwrap();
                assert_eq!(output.user_id, Some("user123".to_string()));
            }
            IR::Problem(_) => panic!("Expected success"),
        }

        server_handle.abort();
        let _ = std::fs::remove_file(socket_path);
    }

    #[tokio::test]
    async fn test_handler_panic() {
        let socket_path = "/tmp/test_panic.sock";
        let _ = std::fs::remove_file(socket_path);

        let handler = |input: TestInput, _ctx: Context| async move {
            if input.value == 999 {
                panic!("Intentional panic for testing");
            }
            Ok(TestOutput {
                result: input.value,
            })
        };

        let server_handle = {
            let socket = socket_path.to_string();
            tokio::spawn(async move {
                let listener = UnixListener::bind(&socket).unwrap();
                let svc = action::function_runner_service_server::FunctionRunnerServiceServer::new(
                    crate::MyService {
                        handler,
                        _marker: PhantomData,
                    },
                );
                tonic::transport::Server::builder()
                    .add_service(svc)
                    .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
                    .await
                    .unwrap();
            })
        };

        let mut client = create_test_client(socket_path).await.unwrap();

        let request = InvokeRequest {
            payload: serde_json::to_vec(&TestInput { value: 999 }).unwrap(),
            metadata: HashMap::new(),
        };

        let response = client.invoke(request).await;
        assert!(response.is_err());

        let err = response.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);

        server_handle.abort();
        let _ = std::fs::remove_file(socket_path);
    }

    #[tokio::test]
    async fn test_multiple_sequential_invocations() {
        let socket_path = "/tmp/test_sequential.sock";
        let _ = std::fs::remove_file(socket_path);

        let handler = |input: TestInput, _ctx: Context| async move {
            Ok(TestOutput {
                result: input.value + 1,
            })
        };

        let server_handle = {
            let socket = socket_path.to_string();
            tokio::spawn(async move {
                let listener = UnixListener::bind(&socket).unwrap();
                let svc = action::function_runner_service_server::FunctionRunnerServiceServer::new(
                    crate::MyService {
                        handler,
                        _marker: PhantomData,
                    },
                );
                tonic::transport::Server::builder()
                    .add_service(svc)
                    .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
                    .await
                    .unwrap();
            })
        };

        let mut client = create_test_client(socket_path).await.unwrap();

        // Make multiple requests
        for i in 0..5 {
            let request = InvokeRequest {
                payload: serde_json::to_vec(&TestInput { value: i }).unwrap(),
                metadata: HashMap::new(),
            };

            let response = client.invoke(request).await.unwrap();
            let result = response.into_inner();

            match result.result.unwrap() {
                IR::Success(success) => {
                    let output: TestOutput = serde_json::from_slice(&success.output).unwrap();
                    assert_eq!(output.result, i + 1);
                }
                IR::Problem(_) => panic!("Expected success"),
            }
        }

        server_handle.abort();
        let _ = std::fs::remove_file(socket_path);
    }

    #[tokio::test]
    async fn test_async_handler_with_delay() {
        let socket_path = "/tmp/test_async.sock";
        let _ = std::fs::remove_file(socket_path);

        let handler = |input: TestInput, _ctx: Context| async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(TestOutput {
                result: input.value * 3,
            })
        };

        let server_handle = {
            let socket = socket_path.to_string();
            tokio::spawn(async move {
                let listener = UnixListener::bind(&socket).unwrap();
                let svc = action::function_runner_service_server::FunctionRunnerServiceServer::new(
                    crate::MyService {
                        handler,
                        _marker: PhantomData,
                    },
                );
                tonic::transport::Server::builder()
                    .add_service(svc)
                    .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
                    .await
                    .unwrap();
            })
        };

        let mut client = create_test_client(socket_path).await.unwrap();

        let request = InvokeRequest {
            payload: serde_json::to_vec(&TestInput { value: 7 }).unwrap(),
            metadata: HashMap::new(),
        };

        let response = timeout(Duration::from_secs(2), client.invoke(request))
            .await
            .unwrap()
            .unwrap();

        let result = response.into_inner();
        match result.result.unwrap() {
            IR::Success(success) => {
                let output: TestOutput = serde_json::from_slice(&success.output).unwrap();
                assert_eq!(output.result, 21);
            }
            IR::Problem(_) => panic!("Expected success"),
        }

        server_handle.abort();
        let _ = std::fs::remove_file(socket_path);
    }
}
