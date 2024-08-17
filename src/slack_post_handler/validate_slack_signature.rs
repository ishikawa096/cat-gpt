use hmac::{Hmac, Mac};
use lambda_http::http::HeaderMap;
use sha2::Sha256;

// https://api.slack.com/authentication/verifying-requests-from-slack
pub fn validate_slack_signature(
    headers: &HeaderMap,
    body: &str,
    slack_signing_secret: &str,
) -> bool {
    type HmacSha256 = Hmac<Sha256>;
    let signature_header = "X-Slack-Signature";
    let timestamp_header = "X-Slack-Request-Timestamp";

    let signature = headers
        .get(signature_header)
        .expect(format!("{} missing", signature_header).as_str())
        .to_str()
        .expect(format!("{} parse error", signature_header).as_str());
    let timestamp = headers
        .get(timestamp_header)
        .expect(format!("{} missing", timestamp_header).as_str())
        .to_str()
        .expect(format!("{} parse error", timestamp_header).as_str());
    let basestring = format!("v0:{}:{}", timestamp, body);

    // Slack Signing SecretをkeyとしてbasestringをHMAC SHA256でhashにする
    let mut mac = HmacSha256::new_from_slice(slack_signing_secret.as_bytes())
        .expect("Invalid Slack Signing Secret");
    mac.update(basestring.as_bytes());
    let expected_signature = mac.finalize();

    // expected_signatureとsignatureが一致するか確認する
    let expected_signature_str = hex::encode(expected_signature.into_bytes());
    return format!("v0={}", expected_signature_str) == signature;
}
