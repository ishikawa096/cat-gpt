# CatGPT

- chatbot for slack

## TODO

- ロジックの切り出し、リファクタリング(どうしていけばいいのか rust の勉強から)
- lambda 以外の hosting に対応
- cargo lambda でサクッと作りたかったが docker コンテナ内に cargo lambda がうまく入らなくて渋々ローカルマシン上で zip ファイルを build してデプロイしている。
  ツール活かせていないのでうまく設定したい。

## 参考にさせていただきました

- Slack で動く ChatGPT のチャットボットを Google Apps Script（GAS）でサクッと作ってみる
  https://zenn.dev/lclco/articles/712d482d07e18c
- Azure Functions と ChatGPT API で作った Slack Bot をコンテキスト対応しました
  https://zenn.dev/jtechjapan/articles/3579c91093c833
