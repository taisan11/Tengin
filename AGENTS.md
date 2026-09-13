# AGENTS.md — Tengin

Tengin は Rust 製の小さな JavaScript エンジンです。Oxc (`oxc_parser` / `oxc_ast`) で
パースし、tree-walking インタプリタで評価します。

## プロジェクト構成

- `src/` — コアクレート `tengin`（パーサ変換、AST、インタプリタ、ビルトイン群）
- `tengin-cli/` — CLI 実行バイナリ
- `tengin-test262/` — test262 コンformance ラナー（`main.rs` がランナー、`lib.rs` が harness/frontmatter 解析ライブラリ）
- `tests/smoke.rs` — 統合スモークテスト
- `vendor/test262/` — test262 の git submodule（**読み取り専用として扱うこと。編集・コミットしない**）

## よく使うコマンド

```sh
cargo build                    # ワークスペース全体をビルド
cargo test                     # ユニットテスト + スモークテスト（通常の検証はこれ）
cargo run -p tengin-cli        # CLI で JS を実行
```

## test262 の扱い — 重要な制約

**test262 をフルで回すこと、また複数回実行することを禁止します。**

- `vendor/test262/test` 以下は数万ファイルあり、フルラン（フィルタ無しの
  `cargo run -p tengin-test262`）は非常に時間がかかります。**絶対にフィルタ無しで実行しないでください。**
- 同じセッション内で test262 を**複数回実行しないでください**。結果が変わらないなら
  再実行しても情報は増えません。1 回の実行で十分です。
- test262 を検証に使う場合のルール:
  1. まず `cargo test`（ユニット/スモークテスト）で検証する。そもそも test262 が不要な変更なら実行しない。
  2. どうしても test262 が必要な場合は、**単一のディレクトリまたは単一ファイルに絞る**
     フィルタを必ず付けて、**1 回だけ**実行する:

     ```sh
     cargo run -p tengin-test262 -- test/language/types/number -v
     cargo run -p tengin-test262 -- S15.7.3.1_A1 -v
     ```

  3. フィルタを変えての再実行・再挑戦の積み重ね（実質的な複数回実行）もしない。
     失敗が見えたら個々のケースの中身を読んで解析し、修正後に `cargo test` で
     検証してから、最後に 1 回だけ該当フィルタで確認する。
  4. `--timeout` を明示的に指定し、ハングしたまま放置しない。
- test262 の失敗リスト全体をダンプして総括するような使い方もしない（フルランに準じる）。

## テストの書き方

- エンジン動作のテストは `Engine::eval` を使う統合テスト（`tests/` 以下または
  `src/` 内の `#[cfg(test)]` モジュール）に書く。`Value` を直接比較する。
- test262 の harness (`assert.sameValue` など) はグローバルに登録されているので
  テストからも利用できるが、基本は `Value` 比較で検証する。
- 新機能を追加したら、対応するテストも必ず追加する。
- テストは速く・独立していること（外部ネットワークやフル test262 ランに依存しない）。

## コードの慣習

- `no_std` は廃止済みで `std` クレート。ただし `src/value.rs` など一部に
  `alloc` import が残っているので、既存スタイルに合わせる。
- ビルトインは `src/builtins/` にモジュール単位で追加し、`src/builtins/mod.rs` の
  `install` で `register_global` する。
- コミット前に `cargo test` が通ることを確認する。
