# app 内置 dev workflow 源

编译期由 `include_str!` 读入(`agent/workflow/builtin.rs`),变成二进制常量。
这是 app 内置能力的 source of truth。

项目级覆盖示例在仓库根 `.everlasting/workflow/dev/`,二者内容需保持同步。
修改本目录后无需改代码(`include_str!` 路径固定),重新编译即生效。
