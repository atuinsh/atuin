<p align="center">
 <picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/atuinsh/atuin/assets/53315310/13216a1d-1ac0-4c99-b0eb-d88290fe0efd">
  <img alt="Text changing depending on mode. Light: 'So light!' Dark: 'So dark!'" src="https://github.com/atuinsh/atuin/assets/53315310/08bc86d4-a781-4aaa-8d7e-478ae6bcd129">
</picture>
</p>

<p align="center">
<em>魔法のシェル履歴</em>
</p>

<hr/>

<p align="center">
  <a href="https://github.com/atuinsh/atuin/actions?query=workflow%3ARust"><img src="https://img.shields.io/github/actions/workflow/status/atuinsh/atuin/rust.yml?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/v/atuin.svg?style=flat-square" /></a>
  <a href="https://crates.io/crates/atuin"><img src="https://img.shields.io/crates/d/atuin.svg?style=flat-square" /></a>
  <a href="https://github.com/atuinsh/atuin/blob/main/LICENSE"><img src="https://img.shields.io/crates/l/atuin.svg?style=flat-square" /></a>
  <a href="https://discord.gg/Fq8bJSKPHh"><img src="https://img.shields.io/discord/954121165239115808" /></a>
  <a rel="me" href="https://hachyderm.io/@atuin"><img src="https://img.shields.io/mastodon/follow/109944632283122560?domain=https%3A%2F%2Fhachyderm.io&style=social"/></a>
  <a href="https://twitter.com/atuinsh"><img src="https://img.shields.io/twitter/follow/atuinsh?style=social" /></a>
  <a href="https://actuated.dev/"><img alt="Arm CI sponsored by Actuated" src="https://docs.actuated.dev/images/actuated-badge.png" width="120px"></img></a>
</p>


[English] | [简体中文] | [日本語]


Atuinは、既存のシェル履歴をSQLiteデータベースに置き換え、コマンドの追加コンテキストを記録します。さらに、Atuinサーバーを介して、マシン間で履歴を完全に暗号化して同期するオプションも提供します。




<p align="center">
  <img src="demo.gif" alt="animated" width="80%" />
</p>

<p align="center">
<em>終了コード、実行時間、時刻、コマンドが表示されます</em>
</p>





検索UIに加えて、次のようなこともできます：

```
# 昨日の午後3時以降に記録された、すべての成功した `make` コマンドを検索
atuin search --exit 0 --after "yesterday 3pm" make
```

私がホストするサーバーを使用するか、自分でホストすることができます！または、同期をまったく使用しないこともできます。すべての履歴同期は暗号化されているため、私がアクセスすることはできませんし、アクセスしたいとも思いません。

## 特徴

- `ctrl-r` と `up` を全画面履歴検索UIに再バインド（設定可能）
- シェル履歴をsqliteデータベースに保存
- **暗号化された**シェル履歴のバックアップと同期
- ターミナル、セッション、マシン間で同じ履歴
- 終了コード、cwd、ホスト名、セッション、コマンドの実行時間などを記録
- 「最も使用されたコマンド」などの統計を計算
- 古い履歴ファイルは置き換えられません
- <kbd>Alt-\<num\></kbd> で以前の項目にクイックジャンプ
- ctrl-rでフィルターモードを切り替え; 現在のセッション、ディレクトリ、またはグローバルから履歴を検索
- コマンドを実行するにはEnter、編集するにはTabを押します

## ドキュメント

- [クイックスタート](#quickstart)
- [インストール](https://docs.atuin.sh/guide/installation/)
- [同期の設定](https://docs.atuin.sh/guide/sync/)
- [履歴のインポート](https://docs.atuin.sh/guide/import/)
- [基本的な使い方](https://docs.atuin.sh/guide/basic-usage/)
## 対応シェル

- zsh
- bash
- fish
- nushell
- xonsh

## コミュニティ

### フォーラム

Atuinにはコミュニティフォーラムがあります。サポートやヘルプはこちらで質問してください: https://forum.atuin.sh/

### Discord

AtuinにはコミュニティDiscordもあります。こちらから参加できます: [here](https://discord.gg/jR3tfchVvW)

# クイックスタート

これにより、Atuin Cloud同期サーバーにサインアップします。すべてがエンドツーエンドで暗号化されているため、秘密は安全です！

オフラインセットアップ、自分でホストするサーバーなどの詳細については、[ドキュメント](https://docs.atuin.sh)を参照してください。

```
curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh | sh

atuin register -u <USERNAME> -e <EMAIL>
atuin import auto
atuin sync
```

その後、シェルを再起動してください！

> [!NOTE]
>
> **Bashユーザー向け**: 上記の手順は必要なフックのために `bash-preexec` を設定しますが、
> `bash-preexec` には制限があります。詳細については、
> [Bash](https://docs.atuin.sh/guide/installation/#installing-the-shell-plugin)
> シェルプラグインのドキュメントのセクションを参照してください。

# セキュリティ

セキュリティ上の問題を発見した場合は、ellie@atuin.sh に通知していただけるとありがたいです。

# 貢献者

<a href="https://github.com/atuinsh/atuin/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=atuinsh/atuin&max=300" />
</a>

[contrib.rocks](https://contrib.rocks)で作成されました。

[English]: ../../README.md
[简体中文]: ../zh-CN/README.md
[日本語]: ./README.md
