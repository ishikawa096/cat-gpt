# CatGPT

- chatbot for slack

## Build

- sam build

## Deploy

- profile slack-bot の場合
  sam deploy --guided --profile slack-bot --capabilities CAPABILITY_NAMED_IAM

## TODO

- ロジックの切り出し、リファクタリング
- lambda 以外の hosting に対応

## 参考にさせていただきました

- Slack で動く ChatGPT のチャットボットを Google Apps Script（GAS）でサクッと作ってみる
  https://zenn.dev/lclco/articles/712d482d07e18c
- Azure Functions と ChatGPT API で作った Slack Bot をコンテキスト対応しました
  https://zenn.dev/jtechjapan/articles/3579c91093c833
