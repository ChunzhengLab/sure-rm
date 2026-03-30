# sure-rm

[English](README.md)

`sure-rm` 是一个用 Rust 编写的命令行工具，使用习惯和系统自带的 `rm` 基本一致。

它不会直接把文件从磁盘彻底删除，而是先将目标移入一个可恢复的回收站。只有您明确要求时，它才会永久删除文件。

## 安装

```sh
brew tap ChunzhengLab/tap
brew install sure-rm
```

详见 [homebrew-tap](https://github.com/ChunzhengLab/homebrew-tap)。

## 模式

`--mode` 和环境变量 `SURE_RM_MODE` 支持三种模式：

- `auto`：根据 TTY 自动判断
- `interactive`：对风险操作额外确认一次
- `batch`：不额外确认，直接执行

**推荐配合 shell alias 使用 `interactive` 模式：**

```sh
alias rm='sure-rm --mode interactive'
```

或者通过环境变量设置默认模式：

```sh
export SURE_RM_MODE=interactive
alias rm='sure-rm'
```

**这样日常 `rm` 就是 sure-rm，需要真正删除时用 `rm -s ...`（或 `rm --sure ...`）回退到系统命令。**

> **注意：** Shell alias 只在交互式终端中生效，脚本中的 `rm` 仍然是系统的 `/bin/rm`，不会影响现有的自动化脚本。

## 功能

安全删除文件、符号链接和目录，默认移入回收站而非直接删除。

### 子命令

- `list` 查看回收站中的所有条目
- `restore` 恢复指定条目，支持按 id 或按原始路径恢复
- `purge` 彻底清理回收站，支持按 id 清理单条或 `--all` 清理全部
- `unlink` 单文件删除入口：`sure-rm unlink [--] <path>`

### 选项

- 支持 `rm` 几乎全部选项：`-d`、`-f`、`-i`、`-I`、`-P`、`-r/-R`、`-v`、`-x`
- `-P` 永久删除，跳过回收站
- `-s` / `--sure` 绕过 sure-rm，直接调用系统 `/bin/rm` 或 `/bin/unlink`
- `--mode auto|interactive|batch` 控制确认行为，也可通过 `SURE_RM_MODE` 环境变量设置

### 安全防护

- 自动拦截 `/`、`.`、`..`、当前目录、`HOME` 等危险路径，防止误删

## 示例

```sh
# 设置 alias
alias rm='sure-rm --mode interactive'

rm -rv build                           # 将 build/ 移入回收站，输出详情
rm -sf build                           # 绕过 sure-rm，执行 /bin/rm -f build
rm list                                # 列出回收站中的所有条目
rm restore 1774864212-68302-250054000  # 按 id 恢复指定条目
rm restore ./notes.txt                 # 按相对路径恢复
rm restore ../docs/notes.txt           # 跨目录相对路径也可以
rm restore /home/user/notes.txt        # 按绝对路径恢复
rm -Pv old.log                         # 永久删除，不进回收站
rm unlink -- -file                     # unlink 一个名为 "-file" 的文件
```

## 回收站

回收站默认位于 `~/.sure-rm`。

```sh
rm list                                # 查看回收站内容
rm restore ./notes.txt                 # 恢复文件
rm purge 1774864212-68302-250054000    # 彻底删除某条记录
rm purge --all                         # 清空回收站
```

在测试或沙箱环境下，可以通过环境变量覆盖回收站路径：

```sh
SURE_RM_ROOT=/tmp/sure-rm sure-rm -rv some-directory
```

## 灵感来源

受 [jwanLab](https://github.com/jwanLab) 启发——她花了数月时间构建了一个不可思议的项目，然后用不到一秒的时间把它删除了。
