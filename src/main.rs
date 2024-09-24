use crate::slack_post_handler::handle_request::handle_request;
use lambda_http::{run, service_fn, Body, Error, Request, Response};
pub mod constants;
pub mod openai;
pub mod slack_post_handler;

// slackからのリクエストを受け取る
async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    let response_body = handle_request(event).await;

    let resp = Response::builder()
        .status(200)
        .header("content-type", "text/html")
        .body(Body::from(response_body))
        .map_err(Box::new)?;
    Ok(resp)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        // disable printing the name of the module in every log line.
        .with_target(false)
        // disabling time is handy because CloudWatch will add the ingestion time.
        .without_time()
        .init();

    run(service_fn(function_handler)).await
}
