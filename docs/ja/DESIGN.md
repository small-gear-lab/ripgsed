# ripgsed 設計ノート

**状態：設計文書。sed 互換のコア、grep モード、`-P`（PCRE2）、Rust ライブラリは実装済みで、実験的な 0.1 として公開しています。** 逐次 VM、正規表現エンジン、自動並列実行があります（実測結果は [ARCHITECTURE.md](ARCHITECTURE.md) と README を参照）。以下に書いたエディタ統合、`--explain`、正規表現エンジンの選択はまだありません。まだ作っていない部分のモジュール名、CLI オプション、型、拡張構文は提案であり、変わる可能性があります。「推奨」と書いた記述は現時点の方針であり、確定した決定ではありません。

## 読み方

- sed 互換のコアは作られていて、項目ごとに GNU sed 4.9 と照らして確認されています（`compat/sed.tsv`、[COMPATIBILITY.md](COMPATIBILITY.md)）。そのために必要だった決定は [DECISIONS.md](DECISIONS.md) にあります。下の「決める」項目は、まだ作っていない部分（エディタ連携、`--explain`、エンジンの選択）のものです。それぞれ、作る前に決めて、範囲と受け入れ基準をリポジトリに記録するべきです。
- 優先度：**P0** 逐次コア、**P1** 正しさと公開境界、**P2** 最適化、**P3** 拡張・連携。優先度のラベルは「実装済み」を意味しません。

## 目的と非目標

### 目的

1. sed の言語を保ちつつ、構文・意味論・実行方式を分離する。
2. 行の意味論を維持しながら、バッファ付き I/O、チャンク処理、安全な並列化の余地を残す。
3. 一つのコアを CLI、Rust API、Vim/Neovim から使えるようにする。
4. GNU sed との差分、実行計画の理由、資源上限を観察できるようにする。
5. GNU sed の drop-in な代替になる。有効な GNU sed の呼び出しのどれに対しても、同じ stdout、終了状態、ファイルへの影響を返す。これは任意の目標ではなく、このプロジェクトの最低要件である（[DECISIONS.md](DECISIONS.md) の D4）。grep モードは、GNU grep について同じことを目指す。作業は段階的に進め（P0 と後述の互換性の管理を参照）、GNU のツールと照合して検証するまで、その段階を達成したとは主張しない。

### 初期の非目標

- ripgrep との互換、すべてのスクリプトの並列化。（GNU sed の完全互換は非目標ではなく要件である。P0 はその途中の最初のマイルストーンにすぎず、製品の範囲ではない。）
- PCRE2 を GNU sed 正規表現の等価な代替として扱うこと。
- エディタ UI の再実装、および測定なしに既存ツールより速いと主張すること。
- 最初から mmap / SIMD / JIT / 非同期 I/O を導入すること。

## P0：最初の縦切り実装

**推奨する範囲：** これは作業の順序であり、製品の範囲を狭めるものではありません。LF 区切りの入力に対するリテラルな一行置換 `s/old/new/g` を、stdin から stdout へ動かすところから始めます（バイト列の方針は決定済み、D2）。これは sed 文法全体をすでに受理するという意味ではありません。未対応の構文を黙って別の意味に解釈してはならず、実装するまではコンパイル時エラーとします。

### 作業の順序

1. CLI の入力契約とエラー分類。
2. ソース位置付きの AST へパースする。
3. 置換と自動出力を備えた逐次 VM。
4. stdout と終了状態を GNU sed と比較する fixture。
5. `-n`、`p`、`d`、正規表現アドレス、`-e`、`-f` を段階的に加える。
6. 分岐と複数行コマンドを加える前に、pattern/hold space とサイクルを安定させる。

### P0 の完了条件

- 合意した部分集合が、すべての fixture で参照出力と一致する。
- fixture は、空入力、空行、最終改行なし、複数マッチ、非マッチ、壊れた構文を含む。
- 未対応構文はコンパイル時エラーとし、パニックせず、成功扱いにもならない。
- ストリーム入力で、メモリ使用量がファイル全体のサイズに比例して増えない（単一レコードの長さに依存する分は明記する）。
- broken pipe とその他の I/O エラーの扱いを定め、実行結果と CLI の終了状態の両方をテストする。

## アーキテクチャ

```text
CLI / Rust API / Vim adapter
             │
      script + options
             ▼
         Parser ─────→ diagnostics + source spans
             │ AST
             ▼
       Effect analysis
             │ proven properties / unknown reasons
             ▼
          Planner
          ┌──┴──────────────────┐
          ▼                     ▼
    Sequential VM          Parallel map
    reference semantics    approved subset only
          │                     │ numbered chunks
          │                Ordered merge
          └─────────┬───────────┘
                    ▼
               Output writer

Shared: regex backend / buffered input / limits / cancellation
```

| モジュール    | 担当すること                         | 担当しないこと            |
|----------|--------------------------------|--------------------|
| syntax   | トークン化、AST、ソース位置、構文診断           | 実行、ファイル書き込み        |
| regex    | 方言ごとのコンパイルとマッチ                 | エンジン間の黙示的な意味変更     |
| analysis | 状態・I/O・制御フローの依存情報              | 解析不能なコードを安全と認定すること |
| plan     | 実行方式の選択と、その理由の記録               | VM の意味論の再定義        |
| runtime  | サイクル、pattern/hold space、コマンド評価 | CLI 引数の解釈          |
| io       | レコード読み込み、出力、バックプレッシャー          | スクリプト構文            |
| cli      | モード決定、引数検証、終了状態                | 変換ロジックの別実装         |
| adapters | API とエディタの接続                   | コア意味論の複製           |

**推奨：** 初めは単一 crate のモジュール構成で始め、依存境界が固まった時点で workspace に分割します。

## 意味論とデータモデル

### 入出力

- 入力を `String` と決めつけず、任意のバイト列を保持できる表現にします。最初の版を UTF-8 限定にする場合は、GNU sed 互換の対象外であることを明記します。
- レコード末尾の区切りを本文と区別して保持し、最終改行がないことを正規化しません。
- LF、NUL 区切り、CRLF、複数ファイルは、一度に実装せず段階的に対応します。
- メモリ上限は入力バッファだけでなく、pattern space、hold space、出力キュー、正規表現の作業領域を含みます。

### 逐次 VM の状態（スケッチ）

```rust
// Illustrates roles only; not a settled API.
struct VmState {
    pattern: Vec<u8>,
    hold: Vec<u8>,
    named_holds: HashMap<HoldName, Vec<u8>>,
    input_position: InputPosition,
    program_counter: usize,
    substitution_flag: bool,
    range_states: Vec<RangeState>,
    pending_output: Vec<OutputEvent>,
}
```

**決めること：** 自動出力、`d`、`D`、`n`、`N`、`p`、`P`、`q`、`Q` が出力を行うタイミングと遅延出力の順序、`t` フラグがリセットされる条件、範囲の開始・終端の判定、複数ファイルにまたがる行番号と状態の扱い。それぞれ GNU sed の観測できる挙動に照らして仕様化し、サイクルの遷移としてテストします。

## AST と静的解析

### AST に必要なもの

- アドレス：単一行、正規表現、範囲、反転、GNU 拡張の使用有無。
- コマンド：置換とフラグ、出力、削除、分岐、ラベル、hold 操作、ファイル I/O。
- ソース位置：どの `-e`/`-f` 由来か、バイトオフセット、表示用の行・列。
- 正規表現の方言と、コンパイル済み表現への参照。

### 効果情報の例

```text
reads_previous_records
consumes_additional_records
reads_or_writes_hold
uses_range_state
uses_global_position_or_eof
may_terminate_early
performs_external_io
has_unknown_control_flow
```

これは保守的な初期の分類です。各効果の有無だけでは不十分で、コマンドの順序と制御フローも解析する必要があります。

### プラン選択の原則

- 証明できた record-local の部分集合だけを並列候補にする。
- 初期のホワイトリストは副作用のない置換と行抽出に限定し、組み合わせも検証する。
- 不明、未対応、ループ、跨行依存のスクリプトは逐次で実行する。解析の失敗と構文エラーは区別する。
- 入力行に番号を付けても、すべてのコマンドが並列化できるわけではない。
- 小さい入力は逐次を選ぶかもしれないが、閾値は測定してから決める。

### `--explain`（提案）

```text
$ ripgsed --explain 's/old/new/g' input.txt
plan: parallel-map
proof: record-local, no external effects
output: ordered

$ ripgsed --explain '1h; 2,$G' input.txt
plan: sequential
reason: hold space carries state across records
```

**未決：** `--explain` を計画の表示だけで終了するものにするか、実行もするか。この例は計画表示のみとして扱います。出力先は、変換結果の stdout と混ざらない場所にします。

## 自動並列化・順序保証・フォールバック

### 入力分割

レコード境界にそろえたチャンクに番号を付けます。チャンクサイズはバイト数を目安とし、巨大な単一レコードを固定長で分割しません。通常ファイルと stdin で動く経路を基本とし、mmap は後から比較します。

### 出力の再整列

各 worker は、チャンク内の結果を入力順で返します。削除は空出力、複数出力は順序付きのイベント列として扱います。マージ側は次に必要なチャンク番号を待ちます。有界キューとディスパッチの上限により、遅い先頭チャンクの背後で滞留が無制限に増えるのを防ぎます。

### 逐次のままにする条件

| 例                       | 初期の判断          | 理由                                                                          |
|-------------------------|----------------|-----------------------------------------------------------------------------|
| `s/old/new/g`           | 並列候補           | レコード内で完結する部分集合                                                              |
| `1h; 2,$G`              | 逐次             | hold space が行をまたぐ                                                           |
| `N; P; D`               | 逐次             | 読み込みとサイクルが行をまたぐ                                                             |
| 範囲アドレス                  | 並列（状態を推測して）    | 各チャンクは、ライターが最後に取り込んだチャンクが終わった状態から始める。前のチャンクが別の状態で終わったときは、その状態でもう一度実行する      |
| `$`、`$-N`               | 並列             | 各チャンクには、後ろに何レコード続くかを（スクリプトが先読みする数まで）伝える                                     |
| `F`、正規表現が決まっている `s//x/` | 並列             | チャンクは自分の入力を知っている。正規表現は直前に試したもの                                              |
| `q` / `Q`               | 並列             | レコード内で完結する。入力の順で最初の終了が採用され、それより後のチャンクは捨てる                                   |
| `w`、`W`、`s///w`         | 並列             | 各チャンクはファイルごとの書き込みを保持し、ライターが順番にファイルへ入れる。`/dev/stdout` と `/dev/stderr` は逐次のまま |
| `r`、`R`、`e`             | 逐次             | `r` は終わらないかもしれないファイルを流し、`R` はファイルの中の位置を覚え、`e` はコマンドを実行する                    |
| 未知の拡張                   | 拒否、または既知の逐次 VM | 未知の意味論は実行しない                                                                |

「フォールバック」は、出力を一部書き出した後に最初からやり直すことではありません。実行前にプランを選びます。後から適応的な切り替えを導入するなら、状態・副作用・再実行の契約を別途設計します。

**受け入れ条件：** worker 数とチャンク境界によらず、stdout と終了状態が逐次 VM と一致する。外部副作用を含むプランは対象外。キャンセルや I/O 失敗の後にも worker が残らない。

## CLI、モード選択、sed / grep のフラグの衝突

drop-in の代替として、一つのバイナリが両方のツールを兼ね、プログラム名からモードを選びます。

**実装済み（D12、D14）：**

- モードは `argv[0]` の basename で決めます。`sed` なら sed モード、`grep`、`egrep`、`fgrep`、`rgrep` なら grep モードです。インストールは、`sed` や `grep` という名前の symlink またはハードリンクを `PATH` 上に置くことで行います。シェルの alias は使わないので、`sed` や `grep` を呼ぶスクリプト、cron、他のプログラムからも使われます。
- `sed` や `grep` の名前のときは、コマンドラインを GNU のツールと同じように解釈し、エラーの挙動も同じです。ただし 1 つだけ足したものがあります。GNU のツールが不正として拒むオプションには、ここでは意味を持たせることがあります（sed モードの `-P` と `--glibc-compat`）。そのため、GNU のツールで有効なコマンドラインの意味が変わることはありません。
- それ以外の名前（`ripgsed` など）のときは、既定は sed モードで、先頭の `--grep` で明示的に grep モードを選びます（最初の引数に限る）。ripgsed だけにあるオプションは `--help` に載っています。提案だった `--explain`、`--engine`、`--extensions` は作っていません。
- モードが確定したら、残りの引数はそのツールの規則で解析します。`--` 以降はオペランドです。

同じオプション文字が二つのツールで別の意味を持つため、解析の前にモードを確定する必要があります。

| オプション   | sed                | grep      |
|---------|--------------------|-----------|
| `-n`    | 自動出力を抑制            | 行番号       |
| `-i`    | in-place 編集        | 大文字小文字を無視 |
| `-r`    | GNU sed での ERE の別名 | 再帰検索      |
| `-e`    | スクリプト              | パターン      |
| `-f`    | スクリプトファイル          | パターンファイル  |
| `-E`    | ERE                | ERE       |
```sh
# Installed as drop-in replacements (symlinks named sed and grep)
sed 's/old/new/g' input.txt
sed -n '/ERROR/p' app.log
grep -n -i 'error' app.log
grep -- '-deprecated' app.log

# Called by its own name: sed mode by default, --grep selects the grep mode
ripgsed -E -f rules.sed input.txt
ripgsed --grep -n -i 'error' app.log
```

どちらのモードも、段階的に、長期的な GNU のツールとの互換を目指します。GNU のツールと照合して検証するまでは、近似で済ませずに未対応とします。ripgrep のような自動 ignore 規則は目標ではありません。grep モードの `-v`、`-c`、再帰、symlink の扱いなどのオプションは、一つずつ仕様化して検証します。

### 意図的な非互換：バージョン表記とヘルプの文面

GNU のツールとの意図的な違いは、プログラムが自分自身を名乗る方法だけです。それ以外はすべて互換にすることを目指します。

- `ripgsed` という名前のとき、`--version` は `ripgsed X.Y.Z` を出力します。`sed` と `grep` の名前のときは、1 行目の形は GNU に合わせつつ、このプロジェクトの名前を出します（例：`sed (ripgsed) X.Y.Z`）。次の行には、挙動が対象とする GNU のバージョンと、実験的であることを書きます。
- 自分自身の名前として `GNU sed` や `GNU grep` は決して出力しません。`--version` の 1 行目を読んで GNU かどうかを判定するスクリプト（configure 系など）は、未実装かもしれない拡張に頼らず、GNU ではない場合の経路を取ります。`tools/gen-fixtures.sh` が使う参照用の sed も同じ方法で確認しているので、ripgsed が参照実装と取り違えられることはありません。
- バージョン番号は ripgsed 自身のもので、GNU のバージョンに偽装することはありません。
- `--help` は、オプション（ripgsed だけにあるものを含む）と ripgsed のホームページを一覧にします。GNU のツールと同様に、`--version` と `--help` は stdout に出力し、終了状態 0 で終わります。
- エラーメッセージ、オプションの挙動、終了状態は互換のままです。エラーの文面は、プログラム名の接頭辞を除いて GNU sed と一字一句一致させます（D5）。

**決定済み：** 長いオプションの曖昧でない先頭一致は `getopt_long` と同じように受け付けます。`egrep`、`fgrep`、`rgrep` はプログラム名です（grep モードのメッセージは `grep` と名乗ります）。

## 正規表現と GNU sed 互換性

正規表現エンジンは、このプロジェクトのために最初から自前で書きます（[DECISIONS.md](DECISIONS.md) の D7）。Rust の `regex` などのライブラリは GNU sed の BRE/ERE を実装していません。POSIX の左端最長一致、後方参照、GNU 拡張、`--posix` の制限、ロケールの挙動、不正なマルチバイト入力がいずれも異なり、つなぎにしても捨てることになるだけです。仕様は GNU sed 4.9 の観測できる挙動であり、POSIX の文面と異なる箇所も含みます。その挙動が何かは、差分テストが決めます。

パターンを読む構文は方言です。BRE か ERE で、それぞれ GNU 拡張を有効にするか `--posix` のように制限するか（`POSIXLY_CORRECT` の違い、たとえば括弧式の中のエスケープも含む）があり、これにロケールが加わります。方言はコンパイルの入力の一部であり、別のエンジンではありません。

**推奨：** 構文方言（sed スクリプト、BRE、ERE）とエンジン選択を分離します。既定のエンジンが対応しない構文のとき、黙って PCRE2 に切り替えてはいけません。GNU 互換プロファイルと拡張プロファイルで許す意味論を文書化します。

### エンジンの抽象化

このプロジェクト自身のエンジンが既定で、ほかのエンジン（PCRE2、のちの実験）もその隣に置けなければなりません。そのため、ほかのコードはライブラリではなくインターフェースに依存します。[DECISIONS.md](DECISIONS.md) の D3 を参照してください。実際のコードでは、PCRE2 は下のトレイトの裏ではなく、`Regex` の中で自前のエンジンの隣にあります（`Regex::compile_pcre`、D14）。下のトレイトは、検討した形です。

```rust
// Illustrates the shape only; not a settled API.
trait RegexEngine {
    type Regex;
    // `Syntax` carries the dialect: BRE or ERE, GNU or --posix rules, locale.
    fn compile(&self, pattern: &[u8], syntax: Syntax, flags: Flags)
        -> Result<Self::Regex, CompileError>;
    fn capabilities(&self) -> Capabilities;
}

trait Matcher {
    // Search `haystack` from `start`, keeping the text before `start`
    // visible so anchors and word boundaries behave as in the full record.
    fn captures_at(&self, haystack: &[u8], start: usize, caps: &mut Captures)
        -> Result<bool, MatchError>;
}
```

- パターンと検索対象はバイト列です（D2）。`s///g`、空一致、`^` / `\<` に必要なため、前のテキストが見える状態のまま、オフセットから照合を始められます。
- 結果はコピーしたテキストではなくキャプチャのオフセットなので、置換は元のレコードから出力を組み立てます。
- `Capabilities` は、エンジンが保証すること（後方参照、POSIX の左端最長一致、対応する方言、資源の上限）を表します。コンパイラはスクリプトを選ばれたエンジンに照らして検査し、未対応の構文をコンパイル時エラーとして報告します。
- 資源の枯渇は `MatchError` であり、「無一致」ではありません（PCRE2 の節を参照）。
- コンパイルはスクリプトごとに一度行い、再利用します。ディスパッチをジェネリクス、enum、トレイトオブジェクトのどれにするかは測定して決めます。インターフェースはそれに依存しません。
- 適合テストはインターフェースに対して書き、すべてのエンジンで実行します。置き換えるエンジンも、同じ差分 fixture に通る必要があります。

### 互換性の管理

- 「パースできる」と「正しく動く」を別の状態として扱う。
- コマンドごとに、未対応・部分対応・検証済みのいずれかを記録する。
- 空の正規表現、空一致の反復、置換の `&` と後方参照、区切りのエスケープを個別にテストする。
- `LC_ALL=C` のみから始めるか、Unicode の規則を別モードに分けるかを決める。
- `--posix` のような既存のオプション名は、その意味論を実装するまで使わない。

## PCRE2 と ReDoS 対策

PCRE2 は opt-in の拡張です。sed モードでも grep モードでも `-P` で使い、ライブラリは `pcre2-sys` を通してプログラムに組み込みます（D14）。バックトラッキングは入力次第で高コストになり得ます。PCRE2 に対応することは、ReDoS を防げることを意味しません。

**実装済み：** 上限を超えたときのエラー（issue #1）。ライブラリが諦めたマッチは、エラーであって「マッチなし」ではありません。`grep -P` は `grep: FILE: exceeded PCRE's backtracking limit` と出力し、GNU grep と同じく、そのファイルで全体を終了状態 2 で終えます（そのあとのファイルは検索せず、`-s` でも消えません）。sed モードとライブラリは、実行のエラーにします（`ripgsed: exceeded PCRE's backtracking limit`、終了状態 4）。**まだ作っていないもの：** 下の上限のオプションです。ライブラリの既定の上限が効きます。

```sh
# Hypothetical CLI: the options for the limits do not exist. Values are examples, not recommended safe defaults.
ripgsed -P \
  --match-limit=100000 \
  --depth-limit=1000 \
  --heap-limit-kib=8192 \
  's/(?<=user=)[a-z]+/REDACTED/g' app.log
```

- match limit は探索作業量の上限であり、ミリ秒単位のタイムアウトではない。
- depth / heap の制限がどの実行経路で効くかを確認する。JIT でも同じと仮定しない。JIT スタックは別に管理する。
- コンパイル段階とマッチ段階の両方で、資源消費に上限をかける。
- 上限の超過は専用のエラーとし、「無一致」に変換せず、入力位置を報告する。
- 総処理時間を強制するには、キャンセル点を持つ callout かプロセス隔離が必要。待機側のタイムアウトでは FFI の実行は止まらない。
- スクリプト、入力、pattern/hold space、出力、worker キューにも上限を設ける。

**受け入れ条件：** 高コストな入力、上限超過、キャンセル、JIT の有効/無効を個別にテストする。安全な既定値は、測定と運用要件から決める。

## 名前付き hold space

**仮の構文：** `h{header}` / `g{header}` / `x{scratch}`。通常の `h/g/x` は無名の領域を使います。GNU 互換モードではこの拡張を拒否し、明示した拡張モードでのみ許します。

```sh
# Print the saved first line whenever a line containing BEGIN appears
ripgsed -n --extensions=named-hold '
  1h{header}
  /BEGIN/{
    g{header}
    p
  }
' input.txt
```

**決めること：** 名前の字句規則、未初期化の領域、`H/G` の拡張、存在しない名前への `x`、名前数と合計バイトの上限、空の領域への改行追加の規則。初期案は、実行開始時にすべての領域が空であること。動的な生成とエラーの扱いは未決です。

名前を付けても、状態への依存はなくなりません。領域は実行ごとに分離し、worker 間で無条件に共有・複製してはいけません。

## Rust crate API（スケッチ）

```rust
// Not a published API. Types and signatures are for design discussion.
let program = Compiler::new()
    .dialect(Dialect::GnuSed)
    .compile("s/old/new/g")?;
let plan = program.plan(Execution::Auto)?;
program.run(reader, writer, &plan)?;
```

- `Compiler`：方言、正規表現バックエンド、許可する拡張を受け取り、不変の `Program` を生成する。
- `Program`：コンパイル済みの構文。実行状態は持たない。
- `Plan`：解析結果と実行戦略。異なる設定でコンパイルされた `Program` と組み合わせて誤用されないよう、所有関係を設計する。
- `RunContext`（候補）：メモリ上限、キャンセル、I/O の許可、実行統計。
- `RunOutcome`（候補）：終了理由、処理量、正常な早期終了。I/O の失敗や上限超過は結果ではなく `Error` とする。

**決めること：** `Read`/`Write` と `BufRead` の境界、バイト指向、`Send`/`Sync` の要件、エラー型、統計のオーバーヘッド、同じ `Program` の複数回実行。実際の署名は、最小の CLI と同じ処理経路を通して検証します。

## GNU sed との差分テストと fuzz

### 契約

参照する GNU sed のバージョン、OS、ロケールを固定します。同じスクリプトと入力バイト列について、stdout、終了状態、生成・変更されたファイルの内容を比較します。stderr はまずエラーの分類を比較し、文面の完全一致は別の要件とします。

```text
fixture
  script.sed
  input.bin
  args            # optional extra arguments
  profile.json    # reference version, locale, OS
  expected.bin    # generated by running the reference sed
  expected.json   # exit status, file effects
```

### テスト群

- 空入力、空行、最終改行なし、NUL、不正な UTF-8、巨大な行、複数ファイル。
- アドレス、範囲、反転、置換、空一致、後方参照、分岐、hold、N/D/P の組み合わせ。
- パーサー fuzz：任意のバイト列でパニックしない。時間とメモリは制限する。
- 実行 fuzz：受理した部分集合に限り GNU sed と比較する。どちらかの上限到達を不一致と混同しない。
- 最適化 fuzz：worker 数とチャンク境界を変えながら、逐次 VM と並列パスを比較する。
- 見つかった例を最小化して fixture 化し、seed と再現コマンドを残す。
- `e/r/w` を含むスクリプトは、隔離した一時領域と許可ポリシーの下で実行し、ホストの任意のコマンドやファイルに到達させない。

fixture の入力とスクリプトはこのプロジェクト用に書いたもので、期待出力は GNU sed をブラックボックスとして実行して生成します。リポジトリの `tests/README.md` を参照してください。GNU sed 自身のテストコードを複製する場合は、そのライセンスを別途確認します。

## ベンチマークの方針

**実測値はまだありません。** まず結果が等価な workload を作り、そのうえで速度とメモリを測ります。

| 記録対象   | 必須項目                                    |
|--------|-----------------------------------------|
| 処理     | 入力サイズ、行長、マッチ密度、スクリプト、seed、入力の hash      |
| マシン    | CPU、RAM、OS、ストレージ、電源・コア条件                |
| ビルド    | Rust と依存のバージョン、最適化、features、参照ツールのバージョン |
| 実行     | コマンド、worker 数、キャッシュ条件、反復回数              |
| 結果     | wall/CPU 時間、最大 RSS、分布、生データ、出力の一致        |
- 小さい入力での起動・解析コストと、大きい入力でのスループットを分けて測る。
- I/O がボトルネックの場合と CPU がボトルネックの場合を分ける。
- 再整列のコスト、PCRE2 の compile/JIT、入力読み込みを、都合よく除外しない。除外するマイクロベンチマークは区別して明示する。
- GNU sed / grep / ripgrep とは、等価な処理だけを比較する。
- 最速値だけでなく分布を報告し、遅くなるケースも残す。

## Vim / Neovim のバッファ範囲処理

内容と行番号を見ながら編集するインターフェース（Ex モード）はすでにあります。アダプターの価値は、変換を適用する前にプレビューできることです。

```vim
" Transform a range, preview the diff, then apply (proposal)
:'<,'>Ripgsed s/ERROR/WARN/g

" Show matching lines with line numbers in a separate window (proposal)
:%SedG/ERROR/

" Existing Ex global command, for comparison
:%g/ERROR/
```

`SedG` は、ここで定義した提案のコマンドです。既存の `:global` は一致行で Ex コマンドを実行する機能であり、プラグインによる抽出とプレビューとは同じではありません。

### アダプターの契約（提案）

1. 選択範囲を一つの独立したストリームとして渡す。行番号と `$` は範囲内が基準で、範囲外の hold 状態は存在しない。
2. シェル文字列を連結せず、argv と stdin で CLI を呼び出す。推奨は、まず CLI のサブプロセス、FFI は後で。
3. 非同期実行を開始するときに、バッファ ID、範囲、`changedtick` を保存する。
4. 結果と差分をプレビューし、バッファが変更されていたら自動で上書きしない。
5. 明示的な確認があったときだけ、一回の undo 単位で適用する。エラーとキャンセルでは元のバッファを変更しない。
6. バッファ処理用のプロファイルでは `-i` と外部 I/O を拒否する。

**決めること：** エンコーディング、最終改行、結果が複数行になる場合の扱い、`SedG` の結果ウィンドウから元の行へのジャンプ、Vim と Neovim で使う非同期 API。

**受け入れ条件：** 実際のバッファで、日本語、最終改行なし、実行中の編集、削除と複数行化、キャンセル、undo を確認する。

## ライセンスとコードの由来

### 独立実装

互換の挙動を独自のコードで実装するだけでは、GNU sed の GPL は自動的には適用されません。このプロジェクトのライセンスは**まだ決まっていません**（独立実装の候補は MIT / Apache-2.0）。依存ライブラリや、借用するテスト・文書は個別に確認します。

### 既存の GPLv3 コードを使う場合

GNU sed は GPLv3 またはそれ以降です。そのコードを複製・改変して派生物を配布する場合は、適用される GPL の条件に従う必要があります。著作権・ライセンス・無保証の表示を保持し、変更とその日付を明示し、ライセンス文書を添付し、配布形態に応じて対応するソースを提供します。組み合わせた派生著作物を、MIT / Apache-2.0 だけで再配布できると扱ってはいけません。

### このリポジトリでの方針

- GNU sed や GNU grep のソースコードをこのリポジトリにコピーしないこと。読むこともしません。挙動は、ツールを動かして調べました（D12）。ソースを参照する必要があったときは、別の読み手が、観察できる規則だけを渡しました。`reference/` 以下のローカルの clone は追跡しません。PCRE2 は BSD ライセンスなので、読んで移植してかまいません（D14 を参照）。
- テストの期待出力は、GNU sed をコマンドとして実行して生成し、そのテストスイートからはコピーしない。
- 由来はファイル単位で記録する。コピーしたコードに、後から permissive なライセンス表示を付けて解消しようとしない。
- サブプロセスの呼び出し、リンク、コードの複製は別の行為である。実際の構成と配布形態を確認する。

この文書には GNU sed の実装コードは含まれません。

## 未決事項

| 判断            | 推奨する初手                                           | 先送りできる範囲              |
|---------------|--------------------------------------------------|-----------------------|
| PCRE2         | 実装済み：`pcre2-sys` を通した `-P`（D14）。上限を超えたときのエラーつき   | 上限のオプション。純 Rust のエンジン |
| explain       | 計画表示のみ、出力先を固定                                    | 機械可読な schema          |
| worker / チャンク | 作成済み：レコード単位で完結するスクリプトは、順序つきのチャンクの上で並列に実行されます（D9） | 閾値、自動調整               |
| 名前付き hold     | 文法と初期状態を別仕様にする                                   | 拡張コマンド全般              |
| API の安定性      | 実験的な 0.1 として公開済み。1.0 までにインターフェースが変わることがあります      | 互換性保証                 |
| エディタ          | サブプロセスとプレビュー                                     | FFI や独自 UI            |

## 参照資料

- [GNU sed manual](https://www.gnu.org/software/sed/manual/sed.html)
- [GNU sed](https://www.gnu.org/software/sed/)
- [GNU GPL v3](https://www.gnu.org/licenses/gpl-3.0.html)
- [GNU GPL FAQ](https://www.gnu.org/licenses/gpl-faq.html)
- [PCRE2 API](https://www.pcre.org/current/doc/html/pcre2api.html)
- [PCRE2 pattern specification](https://www.pcre.org/current/doc/html/pcre2pattern.html)

実装時には、依拠する参照資料のバージョンを固定してください。
