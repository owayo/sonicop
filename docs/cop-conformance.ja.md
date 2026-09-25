# Cop 別の一致状況

README に載せた [Cop 別の一致状況](../README.ja.md#cop-別の一致状況) の表は、全 609 Cop を両者で有効にし、本家の spec が供給する 37,491 ケースで比べたものです。offense が 1 件残らず一致した Cop だけを完全一致に数えます。このページには、すべての Cop に届かせる方法、別に行った直接照合の結果、既定以外の設定値の計測をまとめます。実在の Ruby プロジェクト 5 つでの比較は [CONFORMANCE.md](../CONFORMANCE.md) にあります。

## すべての Cop に届かせる

609 Cop はすべて記録したケースで発火しますが、そのうち 3 個は専用の run を要しました。`Lint/DeprecatedReference`・`Lint/NameTypo`・`Lint/UnusedPrivateMethod` は `rubydex` のプロジェクトインデックスが無ければ何も報告せず、gem の導入と `AllCops/UseProjectIndex` の有効化が要ります。`Lint/DeprecatedReference` はさらに条件があり、`@deprecated` を持つメソッドの定義クラスを**継承したクラスの中**から呼ぶ必要があります。本家の spec は「インデックス無しでは offense を出さない」という `expect_no_offenses` で始まるので「到達不能」と読みたくなりますが、そうではありません。

「完全一致」を 609 にし、「相違」をゼロにすることが現在の目標です。実コードを足しても届きません。RuboCop が既定で無効にしている 56 Cop と、pending として出荷している Cop の多くは、ツリーがどれだけ大きくても素の実行では発火しないためです。そこに届くのは本家の spec が供給する入力の方で、`tests/fixtures/upstream_spec_capture.jsonl` に記録したケースは **609 Cop すべてに到達します**（実測）。走らせ方には間違えやすい点が 2 つあり、**どちらも失敗せずに表を縮めます**。

- **`TargetRubyVersion` は入力の一部であって、全体の設定ではありません。** 2.7 に固定すると `Style/ArrayIntersect`・`Naming/BlockForwarding`・`Style/ItBlockParameter` ほか 11 個がそもそも発火できません。各ケースはその spec が指定した版で走らせます。
- **ファイル名そのものを見る Cop があります。** `Bundler/*` は `Gemfile`、`Gemspec/*` は `.gemspec`、`Naming/FileName` は名前自体を読みます。全ケースを `.rb` で書き出すと、この 17 Cop が何にも一致しませんでした。

## 本家 spec との直接照合

README の表とは別に、2026-08-29 に本家 Cop spec から直接抽出できた 11,506 ケースを oracle で照合しました。本家が入力を読めなかった 226 ケースとクラッシュした 1 ケースを除き、測定できた範囲では Sonicop の**検出差分・訂正差分ともにゼロ**でした。この結果は README の表には加えていません。直接抽出できるケースが無い Cop が 51 個あり、ディレクティブ系 3 Cop は `--only` では測定不能なため、この掃引だけで全 609 Cop の完全一致を証明できないからです。また、この直接掃引は各例の `TargetRubyVersion` をすべて再現せず、中立的な既定条件で実行しています。そのため、今回変更した Ruby 3.4 依存の 4 Cop は 3.4 で別途比較し、メッセージと位置が本家に完全一致することを確認しました。

## 既定以外の設定値

設定値については別に測っています。既定値でしか一致しない Cop は半分しか実装していないのと同じだからです。`Enforced*` 系の設定を持つ 111 Cop すべてを**既定以外の値**に倒して同じコーパスを流すと、**622,317 件の offense のうち 99.995% が一致**し、発火した 96 Cop のうち 85 個が完全一致です。残るのは 10 Cop（いずれも 17 件以下）と、本家がクラッシュして sonicop が正常に検出する 1 Cop です。内訳は [CONFORMANCE.md](../CONFORMANCE.md) にあります。

どちらの表も `scripts/conformance_table.rb` で再現できます。
