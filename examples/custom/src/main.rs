use noctiforge_sdk_rust::{start, Context, Problem};

#[derive(serde::Deserialize)]
struct Request {
    name: String,
}

async fn handler(req: Request, _: Context) -> Result<String, Problem> {
    Ok(format!("Hello, {}!", req.name))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    start(handler).await
} 
