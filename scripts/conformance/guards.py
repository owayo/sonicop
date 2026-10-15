"""比較スクリプトが「測れていないのに一致」と報告する経路を塞ぐ。

一致は 2 通りの理由で出る。**本当に同じ結果だったときと、両者とも何も測れていない
ときである。** 出力だけを見ると区別が付かない。しかも後者は「差分 0」という一番
欲しい答えの形をしているので、疑う動機が生まれない。**偽陰性が成功に化ける。**

2026-08-16 の 1 日で 3 回踏んだ。

  1. `-c <設定ファイル>` を渡したとき、本家と移植版が別の Ruby バージョンで解析して
     いた。文法が違えば木が違うので比較そのものが成立していないが、出力は「差分 0」
     だった。設定ファイルの置き場所から TargetRubyVersion を解決していたのが原因で、
     移植版側は直っている。**だが「両者が同じバージョンで読んだ」ことは、直したあとも
     誰も確かめていなかった。**
  2. `--only Lint/RedundantCopDisableDirective` を渡した。この cop は `--only` と
     併用できない仕様で、本家は `cannot be used with --only` と言って exit 2 し、
     JSON を 1 バイトも出さない。移植版は 0 件を返す。差分を取るスクリプトは両者の
     空ファイルを比べて **4 コーパス全部に「SAME (0)」と報告した。**
  3. `rcfull.py` が犯人を特定せず先頭から捨て、無実の 114 ファイルが比較から消えた。
     消えたぶんは静かに減るだけで、offense 20,505 件が数字に出ないまま落ちていた。

3 は `rcfull.py` 側で直した。ここは 1 と 2 を、どのスクリプトからも呼べる形で塞ぐ。

5 件目は複製する側にあった。**汚れたコーパスを複製すると「両方とも何もしなかった」が
「一致した」に化ける。** 前の実行が `-A` を戻し忘れたツリーを両者に食わせれば、どちらも
「直すものが無い」で終わってバイト比較は一致する。言えるのは「補正済みを壊さない」ことだけで、
**「クリーンから同じ結果に至る」は測れていない。** 事後に reflog で確認できたのは運で、
事前に止まるほうが確実である (cop-lint の指摘)。

4 件目は質が違う。**`--only` を渡すと本家の挙動そのものが変わる cop がある。**

     def enabled_cop_name?(cop_name, config)
       return true if options[:only]&.include?(cop_name)      # registry.rb:386

  `--only X` は登録簿を X だけに絞るので `Registry#disabled_names(config)` が空になり、
  `CommentConfig#inject_disabled_cops_directives` が**何も注入しなくなる**。設定で無効に
  した cop に `-∞` 行からの disable を入れるのがこの注入で、ディレクティブ 3 cop は
  それを読んで判断している。

  実例: `# rubocop:enable all` だけを書いたファイル。`--only` 付きで測ると本家は
  「余計な enable」と報告し、`--only` 無しでは報告しない (pending と既定無効の cop が
  戻す対象になるため)。**`--only` の出力を正解として実装すると、既定設定での誤検出を
  作り込む。** 実際にそこまで行きかけた。

  1 と 2 は*こちらの*スクリプトの不備なので直せた。**これは本家側の意味が変わるので、
  こちらを直しても消えない。** 測れないと知らせて、別の測り方へ誘導するしかない。

**失敗にしてあるものと警告に留めてあるものの区別には理由がある。**

  - バージョン不一致と JSON が読めないのは **失敗**。黙って通せば数字が嘘になる。
  - 両者とも 0 件は **警告**。単一 cop を小さなコーパスにかければ本当に 0 件はあり得る
    (`Naming/MemoizedInstanceVariableName` は rubocop/rubocop に 1 件も無い)。失敗に
    すると本物の 0 件で止まるので、目に入る形で残すところまでにする。

次にこれを単純化しようとした人へ: **「両者とも空なら一致」は書かないこと。** それが
2 の正体である。
"""

import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time

#: 文法として通らないソース。どの Ruby でも `Lint/Syntax` になり、そのメッセージに
#: 実際に使われたパーサのバージョンが載る。設定を読ませたいので `--stdin` で渡す
#: (ファイルを置くと、コーパスに未追跡ファイルが残って対象ファイル数を狂わせる)。
BROKEN_SOURCE = "def probe(\n"

#: `--stdin` に渡す名前。実在しなくてよいが、`.rb` でないと対象から外れる。
PROBE_NAME = "sonicop-target-ruby-probe.rb"

#: 測定に失敗したときの終了コード。**「差分あり」の 1 とは必ず分けること。** コーパスを
#: 回すループで両者を混ぜると、「測れていない」が「差分が出た」に紛れて見逃される。
#: 本家自身が 0=違反なし / 1=違反あり / 2 以上=実行エラー を使っているので、それに揃える。
MEASUREMENT_FAILED = 2

#: `--only` に渡すと本家の挙動が変わる cop。
#:
#: どれも `CommentConfig` が注入する「設定で無効な cop への `-∞` disable」を読んで判断する
#: ので、登録簿を絞った状態では別の状況を測ることになる。`Lint/RedundantCopDisableDirective`
#: はそれ以前に本家が `cannot be used with --only` で exit 2 するが、理由は同じ層にある。
DIRECTIVE_COPS_UNMEASURABLE_WITH_ONLY = (
    "Lint/RedundantCopDisableDirective",
    "Lint/RedundantCopEnableDirective",
    "Lint/MissingCopEnableDirective",
)


def _die(message):
    """測定が成立していないことを告げて止める。

    Raises:
        SystemExit: 常に。終了コードは [`MEASUREMENT_FAILED`]。

    """
    print(message, file=sys.stderr)
    raise SystemExit(MEASUREMENT_FAILED)


def effective_ruby_version(command, cwd, config_args=(), env=None):
    """その cwd と設定で、実際に使われるパーサの Ruby バージョンを処理系に直接聞く。

    設定ファイル・`.ruby-version`・gemspec の `required_ruby_version` と、解決の
    経路はいくつもある。どれを読んだかを推測するのではなく、**壊れたソースを食わせて
    「どのバージョンとして読んだか」を本人に言わせる。**

    Args:
        command: 起動コマンドの配列 (`["rubocop"]` や `["/path/to/sonicop"]`)。
        cwd: 実行ディレクトリ。設定の解決はここが基準になる。
        config_args: 比較本体に渡すのと同じ設定引数 (`["-c", "x.yml"]` など)。
        env: 環境変数。

    Returns:
        `"3.3"` のような文字列。読み取れなければ None。

    Raises:
        SystemExit: コマンドそのものを起動できなかったとき (`MEASUREMENT_FAILED`)。

    """
    try:
        proc = subprocess.run(
            [*command, *config_args, "--stdin", PROBE_NAME, "-f", "emacs"],
            input=BROKEN_SOURCE,
            cwd=cwd,
            capture_output=True,
            text=True,
            env=env,
            check=False,
        )
    except OSError as error:
        # 起動できないのは「バージョンが読めない」ではなく「そもそも測れない」。
        # ここで traceback を出して終わると原因が埋もれるので、名指しで止める。
        _die(f"{' '.join(command)} を起動できない: {error}")
    for line in (proc.stdout + proc.stderr).splitlines():
        _, marker, rest = line.partition("Using Ruby ")
        if marker:
            return rest.split(" ", 1)[0]
    return None


def assert_config_is_read(rubocop, sonicop):
    """`.rubocop.yml` を**祖先の階層から**拾えることを確かめる。拾えなければ止める。

    **この検査だけが `--force-default-config` を付けない。**掃引はどれも既定設定を
    突き合わせるので、**設定ファイルを探す経路を 1 度も踏まない。**踏まないものは
    壊れても分からない。

    2026-08-17 に実際に壊れていた。`find_config` の打ち切りが

        if project_root.is_none_or(|root| directory == root) { break; }

    となっていて、**Gemfile の無い木では最初の 1 段で止まって**いた。上流の
    `find_file_upwards(DOTFILE, target_dir, project_root)` は
    `break if dir == stop_dir` で、`stop_dir` が nil なら**何とも一致しないので根まで昇る**。
    「止める場所が無い」を「すぐ止まる」と読んだ形である。

    症状は **`.rubocop.yml` がリポジトリ直下でコードが `lib/` にある標準の配置で、
    設定が 1 つも効かない**。`Max` も `Exclude` も `EnforcedStyle` も既定のまま走る。
    **4 コーパスの掃引はどれも通っていた** -- どれも `--force-default-config` だからである。

    使う側は掃引の入口で 1 度呼ぶ。rubocop の起動 2 回ぶんしか掛からない。
    """
    with tempfile.TemporaryDirectory() as root:
        root = pathlib.Path(root)
        (root / "lib").mkdir()
        # `Max: 10` は既定 120 と十分離れているので、既定のままだと 0 件になる。
        (root / ".rubocop.yml").write_text("Layout/LineLength:\n  Max: 10\n")
        (root / "lib" / "deep.rb").write_text("x = 1234567890123456789\n")

        def count(argv):
            done = subprocess.run(
                [*argv, "--only", "Layout/LineLength", "-f", "json", "lib/deep.rb"],
                capture_output=True,
                text=True,
                check=False,
                cwd=str(root),
            )
            try:
                data = json.loads(done.stdout)
            except json.JSONDecodeError:
                _die(
                    f"設定の探索を検査できない: {' '.join(argv)} が JSON を返さなかった"
                    f" ({_first_line(done.stderr)})"
                )
            return sum(len(entry.get("offenses", [])) for entry in data.get("files", []))

        theirs = count([*rubocop, "--cache", "false"])
        # **陽性対照**: 本家がここで 0 を返すなら、検査そのものが効いていない。
        if theirs == 0:
            _die(
                "設定の探索の検査が効いていない: 本家が 1 階層上の .rubocop.yml を"
                " 読んでも 0 件を返した。Layout/LineLength の既定が変わったか、"
                "入力が短すぎる"
            )
        ours = count(sonicop)
        if ours != theirs:
            _die(
                f"`.rubocop.yml` を祖先の階層から読めていない: 本家 {theirs} 件 /"
                f" 移植版 {ours} 件。**この検査だけが --force-default-config を"
                " 付けない**ので、他の掃引が全部緑でもここは壊れうる"
            )


def assert_same_ruby_version(rubocop, sonicop, cwd, config_args=(), env=None):
    """両者が同じバージョンとしてソースを読むことを確かめる。違えば止める。

    Returns:
        両者が一致したバージョン。どちらかを読み取れなかったときは None。

    Raises:
        SystemExit: バージョンが食い違ったとき (`MEASUREMENT_FAILED`)。文法が違えば木が
            違い、そこから先の差分は cop の話ではなくなる。**「差分 0」も同じだけ無意味になる。**

    """
    versions = {
        "rubocop": effective_ruby_version(rubocop, cwd, config_args, env),
        "sonicop": effective_ruby_version(sonicop, cwd, config_args, env),
    }
    if None in versions.values():
        unknown = [name for name, value in versions.items() if value is None]
        print(
            f"  警告: 実効 TargetRubyVersion を読めなかった ({', '.join(unknown)})。"
            " 比較は続けるが、両者が同じ文法で読んでいる保証はない",
            file=sys.stderr,
        )
        return None
    if versions["rubocop"] != versions["sonicop"]:
        _die(
            "実効 TargetRubyVersion が食い違っている: "
            f"rubocop={versions['rubocop']} sonicop={versions['sonicop']}。"
            " 文法が違えば木が違うので、この比較は差分 0 でも意味を持たない。"
            " 設定に AllCops/TargetRubyVersion を明記すること"
        )
    return versions["rubocop"]


def read_offense_json(text, label, returncode=0, stderr=""):
    """Offense の JSON を読む。読めなければ、理由を見せて止める。

    **空の出力を「0 件」として扱わないこと。** 本家は起動に失敗すると JSON を 1 バイトも
    出さずに exit 2 する。それを 0 件と読むと、移植版も 0 件だった場合に「完全一致」に
    化ける。終了コードと stderr は、そのとき唯一残っている手がかりなので捨てない。

    Args:
        text: stdout の中身。
        label: エラーに出す名前 (`"rubocop"` など)。
        returncode: 終了コード。rubocop は 0=違反なし / 1=違反あり / 2 以上=実行エラー。
        stderr: 標準エラー出力。原因はここにしか出ない。

    Returns:
        読み取った JSON。

    Raises:
        SystemExit: 出力が空、JSON として読めない、または終了コードがエラーのとき
            (`MEASUREMENT_FAILED`)。

    """
    if returncode >= 2:
        _die(
            f"{label} が exit {returncode} で終了した。0 件ではなく実行エラーである: "
            f"{_first_line(stderr)}"
        )
    if not text.strip():
        _die(
            f"{label} が何も出力しなかった (exit {returncode})。"
            f" 引数か cop 名を疑うこと: {_first_line(stderr)}"
        )
    # 本家は stdout に注記を混ぜることがある (新 cop の案内、`--cache false` と併用した
    # ときの `-P/--parallel is being ignored`)。そのまま json.load すると
    # `Expecting value: line 1 column 1` で落ちて、原因が見えないまま終わる。
    if not text.lstrip().startswith("{"):
        start = text.find("{")
        if start < 0:
            _die(f"{label} の出力に JSON が無い (exit {returncode}): {_first_line(text)}")
        print(f"  警告: {label} の出力の先頭に前置があった: {text[:start].strip()!r}")
        text = text[start:]
    try:
        return json.loads(text)
    except json.JSONDecodeError as error:
        _die(f"{label} の出力を JSON として読めない (exit {returncode}): {error}")
        raise  # _die が必ず投げるので届かない。型の上で戻り値を確定させるためだけ。


def assert_corpus_is_clean(corpus):
    """複製する前に、コーパスが記録の状態かを確かめる。

    **汚れたツリーを複製すると「両方とも何もしなかった」が「一致した」に化ける。** 前の実行が
    `-A` を回して戻し忘れたツリーを両者に食わせれば、どちらも「直すものが無い」で終わり、
    バイト比較は綺麗に一致する。言えるのは「補正済みを壊さない」ことだけで、**「クリーンから
    同じ結果に至る」は測れていない。** 数字は綺麗に出るので気づく手がかりが無い。

    未追跡ファイルも止める理由になる。コーパスに `.rb` が 1 つ増えれば対象ファイル数が動き、
    そこで出た offense はまるごと差分に化ける。

    Args:
        corpus: コーパスの作業ツリーのパス。

    Raises:
        SystemExit: 汚れていたとき。終了コードは [`MEASUREMENT_FAILED`]。

    """
    proc = subprocess.run(
        ["git", "-C", str(corpus), "status", "--porcelain"],
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        # git 管理下でないコーパスもあり得る。そこは「汚れていない」とも言えないので通す。
        return
    dirty = [line for line in proc.stdout.splitlines() if line.strip()]
    if not dirty:
        return
    listed = "\n".join(f"    {line}" for line in dirty[:10])
    more = f"\n    ... 他 {len(dirty) - 10} 件" if len(dirty) > 10 else ""
    _die(
        f"  測定を止めた: {corpus} が記録の状態ではない ({len(dirty)} 件の変更)。\n"
        f"{listed}{more}\n"
        "  汚れたツリーを複製すると、両者とも「直すものが無い」で終わってバイト比較は一致する。\n"
        "  そこで言えるのは「補正済みを壊さない」だけで、「クリーンから同じ結果に至る」は測れない。\n"
        "  前の実行が -A を戻し忘れた可能性がある。他の担当が測っていないことを台帳で確かめてから\n"
        f"    git -C {corpus} reset --hard && git -C {corpus} clean -fd\n"
        "  他の担当が同じコーパスを見ているなら、代わりに固定 SHA の worktree を生やすこと。\n"
        f"    git -C {corpus} worktree add --detach ~/tmp/conf-pinned/<name> <sha>"
    )


def fingerprint(binary):
    """測定に使ったバイナリを一意に指す短い文字列。**出力に必ず載せること。**

    パスは「どれをビルドし忘れたか」を教えてくれない。worktree を分けていればパスは常に
    違うので、**パスが違うだけでは「別のバイナリを測った」の証明にならない。**

    実例 (cop-lint): 変更前と変更後を比べようとして、`git stash push <file>` が cwd の
    ずれで失敗し、`&&` で後続のビルドが飛んだ。**「変更前」と称して変更後のバイナリを測り、
    数字が完全に一致したので「回帰なし」と読めた。** 同じバイナリを 2 回測っただけだった。
    気づいたのは後続の `git stash pop` が `No stash entries found` と言ったからで、
    **測定結果そのものからは最後まで分からなかった。**

    **そしてハッシュは「どのバイナリか」に答えるが、「誰の修正が入っているか」には答えない。**
    2026-08-17 に、この 2 つを別の問いだと気づかないまま 5 回、60 分の門を古い木で回した。
    同じ日に「まだ再現する」という報告が 2 件出て、どちらも**直した人の枝が未マージ**なだけ
    だった。**指紋を貼っていた側も、未マージ数を報告していた側も、それぞれ正しかった** —
    交差だけ誰も答えていなかった。

    `build.rs` が `<binary の隣>/.sonicop-build-sha` に commit を書くので、在れば併記する。
    受け取った側は `git merge-base --is-ancestor <直したコミット> <sha>` で交差を確かめられる。

    Args:
        binary: 移植版バイナリのパス。

    Returns:
        `<名前> <sha256 の先頭 12 桁> (<mtime>) [commit]`。読めなければ理由を含む文字列。
        commit は `.sonicop-build-sha` が無ければ省く (古いバイナリや tarball ビルド)。

    """
    try:
        digest = hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest()[:12]
    except OSError as error:
        return f"{binary} (読めない: {error.strerror})"
    stamp = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(os.path.getmtime(binary)))
    built = build_commit(binary)
    suffix = f" [{built}]" if built else ""
    return f"{os.path.basename(binary)} {digest} ({stamp}){suffix}"


def build_commit(binary):
    """バイナリを作った commit。`build.rs` が隣に書く。無ければ None。

    **`dirty` が付いていたら、その commit だけでは中身が決まらない。** 作業ツリーに未コミットの
    変更があるまま建てた印で、2026-08-17 のやり直しの半分はこの形だった (commit は合っていて
    作業コピーが先に進んでいた)。**受け取った側は「この commit のもの」として扱ってはいけない。**
    """
    marker = pathlib.Path(binary).parent / ".sonicop-build-sha"
    try:
        return marker.read_text(encoding="utf-8").strip() or None
    except OSError:
        return None


def assert_distinct_binaries(before, after):
    """前後比較に同じバイナリが 2 回渡されていたら止める。

    **前後比較は「2 つが違うもの」を前提にしている。** 同じものを 2 回測れば必ず一致し、
    それは一番欲しい答えの形 (差分 0) をしているので疑う動機が生まれない。ビルドし忘れ、
    `stash` の失敗、退避したはずのパスが上書きされていた、のどれでも起きる。

    **中身で見ること。** パスが違っても内容が同じなら同じ測定であり、パスが同じでも
    ビルドし直せば別の測定になる。

    Args:
        before: 変更前のバイナリのパス。
        after: 変更後のバイナリのパス。

    Raises:
        SystemExit: 2 つの内容が同一だったとき。終了コードは [`MEASUREMENT_FAILED`]。

    """
    marks = [fingerprint(before), fingerprint(after)]
    print(f"  変更前: {marks[0]}\n  変更後: {marks[1]}")
    try:
        same = pathlib.Path(before).read_bytes() == pathlib.Path(after).read_bytes()
    except OSError:
        # 読めないものは fingerprint 側が理由付きで報告している。ここでは止めない。
        return
    if not same:
        return
    _die(
        "  測定を止めた: 前後比較に同じ内容のバイナリが 2 回渡されている。\n"
        f"    {before}\n    {after}\n"
        "  同じものを 2 回測れば必ず一致するので、この比較は「変えていない」を証明しない。\n"
        "  ビルドし忘れか、変更前バイナリの退避が上書きされている。\n"
        "  変更前を残すなら、ビルドする前に cp で別名にして、その名前を測ること。"
    )


def assert_only_is_measurable(only):
    """`--only` で測れない cop が指定されていたら止める。

    `--only X` は `Registry#enabled_cop_name?` の 1 行目で X を無条件に有効と答えさせるため、
    登録簿が X だけになり `disabled_names` が空になる。設定で無効な cop への disable 注入が
    起きなくなるので、**ディレクティブ 3 cop はそこで別の挙動になる。** 出力はもっともらしい
    ので、正解として実装すると既定設定での誤検出を作り込む。

    Args:
        only: `--only` に渡す文字列 (カンマ区切り) か cop 名の列。空なら何もしない。

    Raises:
        SystemExit: 測れない cop が含まれていたとき。終了コードは [`MEASUREMENT_FAILED`]。

    """
    if not only:
        return
    names = only.split(",") if isinstance(only, str) else list(only)
    named = [name.strip() for name in names if name.strip()]
    caught = [name for name in named if name in DIRECTIVE_COPS_UNMEASURABLE_WITH_ONLY]
    if not caught:
        return
    _die(
        "  測定を止めた: " + " / ".join(caught) + " は --only では測れない。\n"
        "  --only X は本家の登録簿を X だけに絞るので (registry.rb:386 の\n"
        "  `return true if options[:only]&.include?(cop_name)`)、Registry#disabled_names が\n"
        "  空になり、設定で無効な cop への `-∞` disable が注入されない。これらの cop は\n"
        "  その注入を読んで判断するので、--only 付きの出力は別の状況の答えである。\n"
        "  代わりに --only を外して素で走らせ、出力から対象 cop の行だけを抜いて比べること。\n"
        "    rubocop --force-default-config --cache false -f json | jq '...cop_name == \"X\"'"
    )


def warn_if_nothing_measured(counts, scope):
    """両者とも 0 件のときに、それが本物か測り損ねかを問い直させる。

    「差分 0」は欲しい答えなので、そこで手が止まる。**両者とも 0 件なら、比較したのは
    2 つの空集合であって、一致したのは中身ではない。**

    Args:
        counts: `{"rubocop": 0, "sonicop": 0}` のような検出件数。
        scope: 何を測っていたか (cop 名やコーパス名)。出力に添える。

    Returns:
        警告を出したかどうか。

    """
    if any(counts.values()):
        return False
    names = " / ".join(f"{name}={value}" for name, value in counts.items())
    print(
        f"  警告: {scope} は両者とも 0 件 ({names})。一致したのは中身ではなく空集合である。"
        " 対象ファイルが 0 だったか、--only に渡した cop 名が効いていない可能性を先に潰すこと",
        file=sys.stderr,
    )
    return True


def check_inspected(data, label):
    """`inspected_file_count` が `target_file_count` に届いているかを見る。

    本家は途中で死んでも `ensure` から整形式の JSON を吐くので、出力だけを見ると正常
    終了と区別が付かない。検査されなかったファイルの offense はまるごと「不足」に化ける。

    Returns:
        取りこぼしがあったかどうか。

    """
    summary = data.get("summary", {})
    inspected = summary.get("inspected_file_count")
    target = summary.get("target_file_count")
    if inspected is None or target is None or inspected == target:
        return False
    print(
        f"  警告: {label} は {inspected}/{target} ファイルしか検査していない。"
        " 未検査ぶんの差は cop の問題ではない (rcfull.py でチャンク実行すること)",
        file=sys.stderr,
    )
    return True


def warn_if_syntax_error(source, label="この入力"):
    """本家が構文エラーと読むソースを、cop の比較に使おうとしていたら知らせる。

    **`--only X` で絞ると `Lint/Syntax` は出力から消える。** 構文エラーのソースでは本家は
    どの cop も走らせないので、「X の報告が無い」が残る。それを「X には除外条件がある」とも
    「移植版に実装差がある」とも読めてしまう。**移植版は tree-sitter が寛容に読むので先へ
    進む**ぶん、必ず「本家は報告しないのに移植版は報告する」の形になり、実装差にしか見えない。

    2026-08-16 の 1 日で 2 人が踏んだ。片方は本家に合わせる実装を 13 通りの一致まで作り込み、
    テストハーネスの構文検査で止まって差し戻した。**そのまま入っていれば、移植版が構文エラーの
    コードを吐くようになっていた。**

        routing (/^replies@/i => :replies)   # 本家の -A の出力。ruby -c は通らない
        routing(/^replies@/i => :replies)    # 移植版の出力。こちらが正しい

    **本家の補正結果が構文エラーになることは実際にある。** その場合の差は「合わせるべきでない
    差」であって、移植版のバグではない。

    Args:
        source: 比較に使うソースのパス、またはソース文字列。
        label: 出力に添える名前。

    Returns:
        構文エラーだったかどうか。判定できなければ False。

    """
    path = pathlib.Path(source) if isinstance(source, (str, pathlib.Path)) else None
    if path is not None and path.is_file():
        argv, text = ["ruby", "-c", str(path)], None
    else:
        argv, text = ["ruby", "-c", "-"], str(source)
    try:
        done = subprocess.run(argv, input=text, capture_output=True, text=True, check=False)
    except OSError:
        return False
    if done.returncode == 0:
        return False
    print(
        f"  警告: {label} は Ruby の構文エラーである ({_first_line(done.stderr)})。"
        " 本家は構文エラーのファイルでどの cop も走らせないので、`--only X` の出力から"
        " X が消えているのは「X が報告しない」ではなく「何も走っていない」である。"
        " --only を外して Lint/Syntax の有無を先に見ること",
        file=sys.stderr,
    )
    return True


def assert_upstream_matches_source(rubocop, corpus_root=None):
    """走らせる本家が、移植元として読んでいるソースと同じ版かを確かめる。違えば止める。

    **本家の版は PATH で暗黙に決まる。**gem 環境が更新されれば、誰にも知らされずに
    別の版が神託になる。移植版はソース (`~/tmp/source/rubocop_rubocop`) を仕様の正本として
    読んでいるので、**その 2 つがずれた時点で、比較は「別の製品との比較」になる。**
    出力からは分からない — offense が少し違うだけで、原因は移植版の側に見える。

    同じ形の事故が 2026-08-17 に 1 件起きている。tree-sitter の `parser.c` を
    `package.json` が pin している版ではなく PATH の CLI で生成し、字句解析器の文字クラスが
    2 文字ぶん変わって死守ラインが 5 件割れた。**どちらも「版が暗黙に決まる」で、
    どちらも出力に出ない。**

    期待値をここに書かない。**移植元のソースに聞く** — 2 箇所に同じ数字を持つと必ずずれる。

    Args:
        rubocop: 起動コマンドの配列 (`["rubocop"]` や `["bundle", "exec", "rubocop"]`)。
        corpus_root: コーパスの置き場所。既定は `CORPUS_ROOT` か `~/tmp/source`。

    Returns:
        一致した版の文字列。ソース側を読めなければ None (警告のみ)。

    Raises:
        SystemExit: 版が食い違ったとき (`MEASUREMENT_FAILED`)。

    """
    fallback = str(pathlib.Path.home() / "tmp" / "source")
    root = corpus_root or os.environ.get("CORPUS_ROOT") or fallback
    source = pathlib.Path(root) / "rubocop_rubocop" / "lib" / "rubocop" / "version.rb"
    try:
        found = re.search(r"STRING\s*=\s*['\"]([^'\"]+)", source.read_text())
    except OSError:
        found = None
    if not found:
        print(
            f"  警告: 移植元の版を読めなかった ({source})。"
            " 走らせている本家が移植元と同じ版である保証がない",
            file=sys.stderr,
        )
        return None
    expected = found.group(1)
    try:
        # **番人自身も、本体と同じ条件で叩く。**`--force-default-config` を付ける。
        # この番人は `verify.sh` から
        # `cd "$CORPUS"` の**後**に呼ばれる。コーパスの `.rubocop.yml` が
        # `rubocop-performance` などのプラグインを要求していると、素の `--version` は
        # `cannot load such file` で落ち、**stdout が空になる**。番人は「走っている版が
        # 読めない」と判断して止まるが、メッセージは `PATH の rubocop を確かめること` と
        # 出るので、原因を PATH に探しに行かせてしまう (2026-08-17、4 コーパス中 3 つで発生)。
        # 比較の本体は最初から `--force-default-config` を渡しているので、版の確認だけが
        # 別の条件で走っていた。**測る条件は、測られる条件と揃える。**
        #
        # そして**止まったときのメッセージが原因を PATH に向けていた。**止まったこと自体は
        # 正しいので、番人を疑う動機が生まれず 30 分溶けた。版が空だったときは
        # **なぜ空なのかを一緒に出す** (下の `_first_line(done.stderr)`)。
        done = subprocess.run(
            [*rubocop, "--force-default-config", "--version"],
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError as error:
        _die(f"{' '.join(rubocop)} を起動できない: {error}")
    running = _first_line(done.stdout)
    if running != expected:
        _die(
            f"本家の版が移植元と違う: 走っているのは {running}、"
            f"移植元のソースは {expected} ({source})。"
            " 別の版を神託にした比較は、差が出ても出なくても意味を持たない。"
            " PATH の rubocop を確かめること。"
            f" 版が空なら起動そのものに失敗している (stderr: {_first_line(done.stderr)})"
        )
    return expected


def assert_source_is_released(version, corpus_root=None):
    """移植元の木が、その版の**リリース時点**かを確かめる。master なら止める。

    **これは `assert_upstream_matches_source` から呼ばない。**用途が違う。

      コーパスとして使う (`fullfmt.sh` / `verify.sh`)
        木は「整形する Ruby コード」でしかない。★ master でも困らない
      仕様の正本として使う (`#37` のように本家の入力を記録として持ち歩く道具)
        ★ master だと入力と神託がずれる。ここだけがこの検査を要る

    2026-08-17 に、この 2 つを 1 つの番人に混ぜて死守ラインを止めかけた。
    **番人を厳しくする前に、誰が呼んでいるかを見ること。**


    **`version.rb` は「次に出す番号」であって、木の実態ではない。** 本家は release の
    直後に版を上げるのではなく、**次の release まで前の番号のまま master が進む。**
    したがって上の版番号比較は、**master のどこにいても通る。**「同じ版番号を名乗って
    いるか」を答えているだけで、「同じコードか」は答えていない。

    2026-08-17 に実際に起きた。`~/tmp/source/rubocop_rubocop` は shallow clone (tag 0 個)
    で HEAD が前日の master、`version.rb` は `1.89.0`、**未リリースの変更が 31 件**あった。
    `config/default.yml` の cop 数は gem より多かった (未リリースの cop が
    `VersionAdded: '<<next>>'` で入っている)。**上の検査は通っていた。**
    差は 1 とは限らず、master が進むほど開く。**数を覚えるのではなく、その場で
    両方を数えて突き合わせること。**

    本家は未リリースの変更を `changelog/` に 1 件 1 ファイルで置き、release で
    `CHANGELOG.md` に畳む。**`changelog/` に README 以外が在れば、その木は release から
    進んでいる。**版番号ではなくこれを見る。

    仕様を読むだけなら master でも困らないが、**`#37` のように本家の入力を記録として
    持ち歩く道具は、入力と神託がずれる。**入力は master、期待値は gem の実出力、
    ラベルは "1.89.0" という組み合わせになり、**ラベルが嘘になる。**
    """
    fallback = str(pathlib.Path.home() / "tmp" / "source")
    root = corpus_root or os.environ.get("CORPUS_ROOT") or fallback
    tree = pathlib.Path(root) / "rubocop_rubocop"
    entries = sorted(
        item.name for item in (tree / "changelog").glob("*.md") if item.name.lower() != "readme.md"
    )
    if not entries:
        return
    _die(
        f"移植元の木が {version} のリリース時点ではない: 未リリースの変更が"
        f" {len(entries)} 件ある ({tree / 'changelog'})。"
        f" 例: {', '.join(entries[:3])}。"
        " version.rb は次に出す番号なので、master のどこにいても版番号の比較は通る。"
        " 仕様を読むだけなら差し支えないが、本家の入力や出力を記録として持ち歩く"
        " 用途では、入力と神託がずれたまま『1.89.0』というラベルが付く。"
        f" v{version} の tag で clone した別の木を用意し、そちらを読むこと"
    )


def _first_line(text):
    stripped = (text or "").strip()
    return stripped.splitlines()[0] if stripped else "(出力は空)"
