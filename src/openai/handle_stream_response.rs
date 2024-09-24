use crate::constants::ERROR_FROM_OPEN_AI_MESSAGE;
use crate::openai::chat_gpt_res_body::ChatGptResBody;
use crate::slack_post_handler::api_client::ApiClient;
use anyhow::Result;
use futures::StreamExt;
use reqwest::Response;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpenAIError {
    #[error("Reading Stream Error: {0}")]
    ReadingStream(String),
}

pub async fn handle_stream_response(
    res: Response,
    api_client: ApiClient,
    bot_message_ts: &str,
) -> Result<()> {
    let mut stream = res.bytes_stream();

    let mut last_update = Instant::now() - Duration::from_secs(1);
    let mut text = String::new();
    let mut last_post_text = String::new();
    // 途切れた文字列を保持する
    let mut partial_str = String::new();
    let mut partial_bytes: Vec<u8> = Vec::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(chunk) => {
                for line in chunk.split(|&c| c == b'\n') {
                    match std::str::from_utf8(line) {
                        Ok(p) => {
                            match p.strip_prefix("data: ") {
                                Some(p) => {
                                    if p == "[DONE]" {
                                        break;
                                    }

                                    let json: ChatGptResBody = match serde_json::from_str(p) {
                                        Ok(val) => val,
                                        Err(_) => {
                                            // jsonに変換できない場合は途切れた文字列として一旦保持する
                                            partial_str = "data: ".to_string() + p;
                                            continue;
                                        }
                                    };

                                    update_message_every_second(
                                        json,
                                        &mut text,
                                        &api_client,
                                        &mut last_update,
                                        &mut last_post_text,
                                        bot_message_ts,
                                    )
                                    .await?;
                                }
                                None => {
                                    // 前回途切れた文字列に結合する
                                    partial_str.push_str(p);

                                    // 完全な文字列になったか確認する
                                    let should_break = update_message_if_complite_string(
                                        &mut partial_str,
                                        &mut text,
                                        &api_client,
                                        &mut last_update,
                                        &mut last_post_text,
                                        bot_message_ts,
                                    )
                                    .await?;

                                    if should_break {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            // UTF-8に変換できない場合はpartial_bytesに追加する
                            partial_bytes.extend_from_slice(line);

                            // 完全な文字列になったか確認する
                            let should_break = update_message_if_complite_bytes(
                                &mut partial_bytes,
                                &mut text,
                                &api_client,
                                &mut last_update,
                                &mut last_post_text,
                                bot_message_ts,
                            )
                            .await?;

                            if should_break {
                                break;
                            }
                        }
                    }
                }
            }
            Err(e) => {
                return Err(OpenAIError::ReadingStream(e.to_string()).into());
            }
        }
    }

    // 未投稿の文がある場合は更新する
    let text_to_post = if text == "" {
        // 文が空の場合はエラー文を投稿する
        ERROR_FROM_OPEN_AI_MESSAGE
    } else {
        text.as_str()
    };
    api_client
        .update_message(text_to_post, bot_message_ts)
        .await?;
    Ok(())
}

async fn update_message_if_complite_string(
    partial_str: &mut String,
    text: &mut String,
    api_client: &ApiClient,
    last_update: &mut Instant,
    last_post_text: &mut String,
    bot_message_ts: &str,
) -> Result<bool> {
    match partial_str.strip_prefix("data: ") {
        Some(ps) => {
            if ps == "[DONE]" {
                return Ok(true); // NOTE: trueの場合はbreakする
            }
            match serde_json::from_str(ps) {
                Ok(val) => {
                    let json: ChatGptResBody = val;

                    update_message_every_second(
                        json,
                        text,
                        &api_client,
                        last_update,
                        last_post_text,
                        bot_message_ts,
                    )
                    .await?;

                    *partial_str = String::new();
                    return Ok(false);
                }
                Err(_) => {
                    // jsonに変換できない場合は次のchunkを待つ
                    return Ok(false);
                }
            }
        }
        None => {
            // "data: "から始まっていない場合は次のchunkを待つ
            return Ok(false);
        }
    }
}

async fn update_message_if_complite_bytes(
    partial_bytes: &mut Vec<u8>,
    text: &mut String,
    api_client: &ApiClient,
    last_update: &mut Instant,
    last_post_text: &mut String,
    bot_message_ts: &str,
) -> Result<bool> {
    match std::str::from_utf8(&partial_bytes) {
        Ok(ps) => {
            match ps.strip_prefix("data: ") {
                Some(ps) => {
                    if ps == "[DONE]" {
                        return Ok(true); // NOTE: trueの場合はbreakする
                    }
                    match serde_json::from_str(ps) {
                        Ok(val) => {
                            let json: ChatGptResBody = val;

                            update_message_every_second(
                                json,
                                text,
                                &api_client,
                                last_update,
                                last_post_text,
                                bot_message_ts,
                            )
                            .await?;

                            // 保持したbyte文字列をクリアする
                            partial_bytes.clear();
                            return Ok(false);
                        }
                        Err(_) => {
                            // jsonに変換できない場合は次のchunkを待つ
                            return Ok(false);
                        }
                    }
                }
                None => {
                    // "data: "から始まっていない場合は次のchunkを待つ
                    return Ok(false);
                }
            }
        }
        Err(_) => {
            // UTF-8に変換できない場合は次のchunkを待つ
            return Ok(false);
        }
    }
}

async fn update_message_every_second(
    json: ChatGptResBody,
    text: &mut String,
    api_client: &ApiClient,
    last_update: &mut Instant,
    last_post_text: &mut String,
    bot_message_ts: &str,
) -> Result<()> {
    let content = json.get_content();
    if content.len() == 0 {
        return Ok(());
    }

    // textに追加
    text.push_str(content.as_str());

    // NOTE: 1秒に1回更新する
    if last_update.elapsed() > Duration::from_millis(1000) {
        *last_update = Instant::now();
        *last_post_text = text.to_string();
        api_client.update_message(text, bot_message_ts).await?;
    }
    Ok(())
}
