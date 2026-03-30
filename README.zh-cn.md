# sure-rm

[English](README.md)

`sure-rm` 是一个用 Rust 编写的命令行工具，接口风格接近 `rm`，但默认采用更安全的删除策略。除非显式要求永久删除，否则它会先将目标移入可恢复的回收目录，而不是直接从文件系统中抹除。

本项目并不追求对系统 `rm` 的逐项复刻，而是在保留常见使用习惯的前提下，将默认行为调整为更稳妥的方案。

## 安装

```sh
brew tap ChunzhengLab/tap
brew install sure-rm
```

详见 [homebrew-tap](https://github.com/ChunzhengLab/homebrew-tap)。

## 功能

- 提供更安全的 `rm` 式删除，适用于文件、符号链接和目录
- 提供 `list`、`restore` 和 `purge` 三个子命令
- 支持 `-d`、`-f`、`-i`、`-I`、`-P`、`-r/-R`、`-v`、`-W` 和 `-x`
- 支持 `--mode auto|interactive|batch`
- 支持 `--sure`，用于绕过 sure-rm，直接调用系统命令
- 支持 `unlink` 形式的入口：
  - `sure-rm unlink [--] <path>`
  - 也可以将二进制以 `unlink` 的名称调用
- 会阻止删除高风险目标，例如 `/`、`.`、`..`、当前工作目录以及 `HOME`

### 与 BSD rm 的差异

| 选项 | BSD rm | sure-rm |
|------|--------|---------|
| `-P` | 无效果（仅为向后兼容保留） | 永久删除，不进入回收目录 |
| `-W` | 通过 union 文件系统 whiteout 恢复 | 从回收目录恢复指定路径最近一次删除的条目 |

## 示例

```sh
sure-rm -rv build
sure-rm --sure -rf build
sure-rm list
sure-rm restore 1774864212-68302-250054000
sure-rm -W ./notes.txt
sure-rm -Pv old.log
sure-rm unlink -- -file
```

## 模式

`--mode` 和 `SURE_RM_MODE` 支持以下取值：

- `auto`：根据 TTY 情况自动判断
- `interactive`：对风险较高的操作默认增加一次确认
- `batch`：不额外增加隐式确认

`interactive` 模式适合配合 shell alias 使用，例如：

```sh
alias rm='sure-rm --mode interactive'
```

在这种配置下，`rm --sure ...` 可以作为回退到系统原生命令的显式开关。

## 回收根目录

默认情况下，回收目录位于 `~/.sure-rm`。

在测试或沙箱环境中，可以通过环境变量覆盖该路径：

```sh
SURE_RM_ROOT=/tmp/sure-rm sure-rm -rv some-directory
```

## 灵感来源

受 [jwanLab](https://github.com/jwanLab) 启发——她花了数月时间构建了一个叹为观止的项目，然后用不到一秒的时间把它删除了。
