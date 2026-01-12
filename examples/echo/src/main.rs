use noctiforge_sdk_rust::{start, Context, Problem};

#[derive(serde::Deserialize)]
struct Request {
    msg: String,
}

fn find_user_by_id(id: u8) -> Option<&'static str> {
    match id {
        1 => Some("ow1"),
        2 => Some("tom"),
        _ => None,
    }
}

async fn handler(req: Request, c: Context) -> Result<String, Problem> {
    if req.msg.is_empty() {
        return Err(Problem {
            r#type: "user_echo/empty_name".to_string(),
            detail: "name is empty".to_string(),
        });
    }

    let user_id: u8 = c
        .values
        .get("UserId")
        .ok_or_else(|| Problem {
            r#type: "user_echo/empty_name".to_string(),
            detail: "name is empty".to_string(),
        })?
        .parse()
        .map_err(|_| Problem {
            r#type: "user_echo/invalid_id".to_string(),
            detail: "UserId must be a valid u8".to_string(),
        })?;

    let name = find_user_by_id(user_id).ok_or_else(|| Problem {
        r#type: "user_echo/user_not_found".to_string(),
        detail: "Could not find the user".to_string(),
    })?;

    Ok(format!("Hello, {}!, your msg is '{}'", name, req.msg))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    start(handler).await
}
