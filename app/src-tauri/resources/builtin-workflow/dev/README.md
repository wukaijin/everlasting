# app 内置 dev workflow 源

编译期由 `include_str!` 读入(`agent/workflow/builtin.rs`),变成二进制常量。
这是 app 内置能力的 source of truth。

项目级镜像在仓库根 `.everlasting/workflow/dev/`,与本目录保持 byte-identical 同步。
同步范围 = workflow.json + agents/ + skills/;本 README 不参与加载,不属于镜像约定(验收用
`diff -r --exclude=README.md`)。
修改本目录后无需改代码(`include_str!` 路径固定),重新编译即生效。
