use lambda_http::{run, service_fn, Body, Error, Request, Response};
use regex::Regex;
use reqwest::header;
use serde_derive::{Deserialize, Serialize};
use std::{error::Error as StdError, process};

#[derive(Deserialize)]
struct Env {
    gpt_model: String,
    bot_member_id: String,
    slack_auth_token: String,
    bot_chanel_id: String,
    openai_secret_key: String,
}

#[derive(Deserialize, Clone, Debug)]
struct SlackMessage {
    text: String,
    thread_ts: Option<String>,

    #[serde(rename = "type")]
    type_name: String,
    subtype: Option<String>,
    user: String,
    channel: String,
    ts: String,
}

#[derive(Deserialize)]
struct SlackHistoryResponse {
    messages: Vec<SlackMessage>,
}

#[derive(Deserialize, Serialize, Debug)]
struct OpenAiReqMessage {
    role: String,
    content: String,
}

#[derive(Serialize, Debug)]
struct ChatGptReqBody {
    messages: Vec<OpenAiReqMessage>,
    model: String,
    temperature: f32,
    // max_tokens: i32,
    // top_p: f32,
    // n: i32,
    // logprobs: i32,
    // echo: bool,
    // stop: Vec<String>,
}

#[derive(Deserialize)]
struct ChatGptChoice {
    message: OpenAiReqMessage,
}

#[derive(Deserialize)]
struct ChatGptResBody {
    choices: Vec<ChatGptChoice>,
}

#[derive(Deserialize)]
struct SlackEvent {
    #[serde(rename = "type")]
    type_name: String,
    event: Option<SlackMessage>,
    challenge: Option<String>,
}

async fn fetch_messages_in_thread(
    channel_id: &str,
    bot_member_id: &str,
    slack_auth_token: &str,
) -> Result<Vec<SlackMessage>, Box<dyn StdError>> {
    let url = format!(
        "https://slack.com/api/conversations.history?channel={}&user={}",
        channel_id, bot_member_id
    );

    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", slack_auth_token).parse().unwrap(),
    );

    let client = reqwest::Client::new();
    let res = client.get(url).headers(headers).send().await?;
    println!("conversations.history: {:?}", res);
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
    let slack_auth_token = &env.slack_auth_token;
    let is_in_thread = trigger_message.thread_ts.is_some();
    let is_mention = trigger_message.text.contains(bot_member_id);

    // スレッド外の場合、botへのmentionかDMの場合のみメッセージを返す
    if !is_in_thread && (is_mention || trigger_message.channel == env.bot_chanel_id.as_str()) {
        return Ok(vec![trigger_message]);
    } else if is_in_thread {
        // bot以外へのメッセージの場合は無視する
        let is_mention_to_other = !is_mention && trigger_message.text.contains("<@");
        if is_mention_to_other {
            return Ok(vec![]);
        }

        let messages_in_thread =
            fetch_messages_in_thread(&trigger_message.channel, bot_member_id, slack_auth_token)
                .await?;
        println!("messages_in_thread: {:?}", messages_in_thread);
        // スレッド内でbotが発言しているかどうか
        let is_bot_involved_thread = messages_in_thread.iter().any(|m| &m.user == bot_member_id);

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

// メンションされたテキストから、メンション部分を除去して返す
fn trim_mention_text(source: &str) -> String {
    let re = Regex::new(r"^<.+> ").unwrap();
    let result = re.replace(source, "");
    result.into_owned()
}

// SlackメッセージをChatGPTのクエリメッセージ形式に変換する
fn parse_slack_messages_to_chat_gpt_quesry_messages(
    messages: Vec<SlackMessage>,
) -> Vec<OpenAiReqMessage> {
    if messages.len() == 0 {
        return vec![];
    }

    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };
    let bot_member_id = &env.bot_member_id;
    messages
        .into_iter()
        .map(|m| {
            let role = if m.user == bot_member_id.as_str() {
                "bot"
            } else {
                "user"
            };
            let content = trim_mention_text(&m.text);
            OpenAiReqMessage {
                role: role.to_string(),
                content: content,
            }
        })
        .collect()
}

async fn create_request_body_for_chat_gpt(
    trigger_message: SlackMessage,
) -> Result<ChatGptReqBody, Box<dyn StdError>> {
    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };
    let messages_asked_to_bot = fetch_messages_asked_to_bot(trigger_message).await?;

    println!("messages_asked_to_bot: {:?}", messages_asked_to_bot);

    let mut messages = parse_slack_messages_to_chat_gpt_quesry_messages(messages_asked_to_bot);
    let personality = "";
    let system_message = OpenAiReqMessage {
        role: "system".to_string(),
        content: personality.to_string(),
    };
    let response = ChatGptReqBody {
        messages: messages.splice(..0, vec![system_message]).collect(),
        model: env.gpt_model,
        temperature: 0.7,
    };
    return Ok(response);
}

// ChatGPTにリクエストを送る
async fn fetch_chat_gpt_response(
    trigger_message: SlackMessage,
) -> Result<String, Box<dyn StdError>> {
    let env = match envy::from_env::<Env>() {
        Ok(val) => val,
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };

    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", env.openai_secret_key).parse().unwrap(),
    );

    let request_body = create_request_body_for_chat_gpt(trigger_message).await?;
    println!("request_body: {:?}", request_body);
    if request_body.messages.len() == 0 {
        return Ok("".to_string());
    }

    let client = reqwest::Client::new();
    let res = client
        .post("https://api.openai.com/v1/chat/completions")
        .headers(headers)
        .json(&request_body)
        .send()
        .await?;

    match res.status().as_u16() {
        200 => {
            let body = res.text().await?;
            let json: ChatGptResBody = serde_json::from_str(&body)?;
            let choices = json.choices;
            if choices.len() == 0 {
                return Ok("応答が空でした".to_string());
            }
            let text = choices[0].message.content.clone();
            return Ok(text);
        }
        429 => return Ok("利用上限に達しました".to_string()),
        _ => {
            return Ok("エラーが発生しました".to_string());
        }
    }
}

// Slackにメッセージを送る
async fn post_slack_message(
    channel_id: &str,
    text: &str,
    slack_auth_token: &str,
    thread_ts: String,
) -> Result<(), Box<dyn StdError>> {
    let url = "https://slack.com/api/chat.postMessage";

    let mut headers = header::HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", slack_auth_token).parse().unwrap(),
    );

    let json = serde_json::json!({
        "channel": channel_id,
        "text": text,
        "thread_ts": thread_ts,
    });

    println!("json: {}", json.to_string());

    let client = reqwest::Client::new();
    let res = client.post(url).headers(headers).json(&json).send().await?;
    match res.status().as_u16() {
        200 => {
            return Ok(());
        }
        _ => {
            return Err(format!("Error: {}", res.status()).into());
        }
    }
}

// Slackイベントに応じて処理
async fn process_slack_event(slack_event: SlackEvent) -> String {
    return match slack_event.type_name.as_str() {
        // Slackの認証(初回のみ)
        "url_verification" => slack_event.challenge.unwrap(),
        "event_callback" => {
            let trigger_message = slack_event.event.unwrap();
            println!("{:?}", trigger_message);
            if trigger_message.type_name != "message" {
                "OK".to_string();
            }
            // メッセージが編集または削除された場合、OKを返して処理を終了する
            if trigger_message.subtype.is_none() {
                "OK".to_string();
            }

            let env = match envy::from_env::<Env>() {
                Ok(val) => val,
                Err(err) => {
                    println!("{}", err);
                    process::exit(1);
                }
            };

            println!("bot_member_id: {}", env.bot_member_id.as_str());

            let user_id = &trigger_message.user;
            // Bot自身によるメッセージである場合、OKを返して処理を終了する
            if user_id == env.bot_member_id.as_str() {
                "OK".to_string();
            }
            let ts = trigger_message.ts.clone();

            // TODO: 処理したメッセージのキャッシュを作る

            // ChatGPTの回答を取得する
            // TODO: ChatGPTの回答が空の場合は、Slackにメッセージを送らない
            let response_text = fetch_chat_gpt_response(trigger_message).await.unwrap();
            println!("{}", response_text);
            if response_text == "" {
                "OK".to_string();
            }
            // SlackにChatGPTの回答を送る
            let post = post_slack_message(
                &env.bot_chanel_id,
                &response_text,
                &env.slack_auth_token,
                ts,
            )
            .await;
            match post {
                Ok(_) => "OK".to_string(),
                Err(_) => "NG".to_string(),
            }
        }
        _ => {
            println!("{}", slack_event.type_name);
            "OK".to_string()
        }
    };
}

// slackからのリクエストを受け取る
async fn function_handler(event: Request) -> Result<Response<Body>, Error> {
    let body_str = match event.body() {
        Body::Text(s) => s,
        _ => "",
    };
    let json: SlackEvent = serde_json::from_str(&body_str).unwrap();
    let response_body = process_slack_event(json).await;

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
