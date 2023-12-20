use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use reqwest::header;
use serde_derive::Deserialize;
use std::{error::Error as StdError, process};

#[derive(Deserialize)]
struct Env {
    gpt_model: String,
    gpt_api_key: String,
    bot_member_id: String,
    bot_auth_token: String,
    bot_chanel_id: String,
    openai_secret_key: String,
}

#[derive(Deserialize)]
struct SlackMessage {
    channel_id: String,
    user_id: String,
    text: String,
    thread_ts: String,
}

#[derive(Deserialize)]
struct SlackHistoryResponse {
    messages: Vec<SlackMessage>,
}

async fn fetch_messages_in_thread(
    channel_id: &str,
    bot_member_id: &str,
    bot_auth_token: &str,
) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    let url = format!(
        "https://slack.com/api/conversations.history?channel={}&user={}",
        channel_id, bot_member_id
    );

    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", bot_auth_token).parse().unwrap(),
    );

    let client = reqwest::Client::new();
    let res = client.get(url).headers(headers).send().await?;

    // エラーハンドリング
    if !res.status().is_success() {
        return Err(format!("Error: {}", res.status()).into());
    }

    let body = res.text().await?;
    let json: SlackHistoryResponse = serde_json::from_str(&body)?;
    return Ok(json.messages);
}

async fn fetch_messages_asked_to_bot(
    trigger_message: SlackMessage,
) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };
    let bot_member_id = &env.bot_member_id;
    let bot_auth_token = &env.bot_auth_token;
    let is_in_thread = trigger_message.thread_ts.parse::<bool>().unwrap();
    let is_mention = trigger_message.text.contains(bot_member_id);

    // スレッド外の場合、botへのmentionかDMの場合は処理を続ける
    if !is_in_thread && (is_mention || trigger_message.channel_id == env.bot_chanel_id.as_str()) {
        // trigger_messageをvecに入れて返す
        return Ok(vec![trigger_message]);
    } else if is_in_thread {
        // bot以外へのメッセージの場合は無視する
        let is_mention_to_other = !is_mention && trigger_message.text.contains("<@");
        if is_mention_to_other {
            return Ok(vec![]);
        }

        let messages_in_thread =
            fetch_messages_in_thread(&trigger_message.channel_id, bot_member_id, bot_auth_token)
                .await?;
        // スレッド内でbotが発言しているかどうか
        let is_bot_involved_thread = messages_in_thread
            .iter()
            .any(|m| &m.user_id == bot_member_id);

        // スレッド内の場合、botへのmentionか、botが発言している場合はメッセージを返す
        if is_mention || is_bot_involved_thread {
            return Ok(messages_in_thread);
        } else {
            return Ok(vec![]);
        }
    } else {
        return Ok(vec![]);
    }
}

/// This is the main body for the function.
/// Write your code inside it.
/// There are some code example in the following URLs:
/// - https://github.com/awslabs/aws-lambda-rust-runtime/tree/main/examples
async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    // Extract some useful information from the request
    let who = event
        .query_string_parameters_ref()
        .and_then(|params| params.first("name"))
        .unwrap_or("world");
    let message = format!("Hello {who}, this is an AWS Lambda HTTP request");

    let personality = "";

    // Return something that implements IntoResponse.
    // It will be serialized to the right response event automatically by the runtime
    let resp = Response::builder()
        .status(200)
        .header("content-type", "text/html")
        .body(message.into())
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
