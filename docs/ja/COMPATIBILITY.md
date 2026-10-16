# 互換性の一覧

Full GNU sed compatibility is the minimum requirement of the project (D4 in [DECISIONS.md](DECISIONS.md)). The matrix in `compat/sed.tsv` lists what that means item by item and records how far each item has got. The grep mode has its own matrix, `compat/grep.tsv` (see [GREP.md](GREP.md)), made the same way from the manual of GNU grep and kept in the same columns.

## 項目の出どころ

- GNU sed マニュアルの構成に沿っています。呼び出し方、オプション、終了状態、スクリプト構文、すべてのコマンド、`s` コマンドとそのフラグ、アドレス、正規表現、エスケープ、ロケールの挙動、実行サイクル、バグではないと明記された挙動です。
- 一覧づくりに使ったのは文書だけです。ソースコードは読んでおらず、コピーもしていません。
- マニュアルがすべてを網羅しているわけではありません。文書化されていない挙動は、差分テストで見つかった時点で、出どころを `tests` として追加します。一覧にない項目も、要件の対象外にはなりません。

## 列

| 列        | 意味                                                |
|----------|---------------------------------------------------|
| `id`     | `CMD-005` のような固定の識別子。再利用しません。                     |
| `area`   | 項目のグループ（`command`、`option`、`address`、`regex` など）。 |
| `item`   | 機能または挙動。                                          |
| `status` | `unsupported`、`partial`、`verified` のいずれか。         |
| `source` | 項目の出どころのマニュアルの節、または `tests`。                      |
| `notes`  | 注意が必要な点、エッジケース、影響する決定。                            |

## 状態の規則

- `unsupported`：通る fixture が一つもない。未対応の構文はコンパイル時エラーで拒否し、近似で済ませることはありません。
- `partial`：その項目を対象とする fixture の一部が通り、一部が通らない。
- `verified`：その項目をカバーする fixture がすべて通る。主張だけで `verified` になることはなく、テスト結果から計算される。fixture は GNU sed 自身の結果（C ライブラリの正規表現マッチャーの癖を含む）を保持しているので、`--glibc-compat`（D10）を付けて実行する。[KNOWN-DIFFERENCES.md](KNOWN-DIFFERENCES.md) を参照。

status 列は手ではなくテストランナー（`RIPGSED_UPDATE_MATRIX=1 cargo test`）が書き換えます。fixture のない項目は状態が変わらず、fixture なしに `verified` にはなれません。`verified` の項目を対象とする fixture は `cargo test` で強制されるので、検証済みの項目が黙って退行することはありません。

## 集計

```sh
tools/compat-summary.sh
```

領域ごと・状態ごとの項目数を表示します。
