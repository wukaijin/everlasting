#!/usr/bin/env bash
# bwrap 沙盒探针矩阵(路线一复现脚本)
# 用途:P3a spike 证据复跑;全部探测非破坏性(只写 /tmp/sbx-test 与沙盒内临时文件)。
# 前置:bwrap 已装(全新 WSL 没有,见 research/generalization.md)。
set -u
PROJ=/tmp/sbx-test/proj
rm -rf /tmp/sbx-test && mkdir -p "$PROJ"

run_sbx() { bwrap --unshare-net --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp --bind "$PROJ" "$PROJ" "$@"; }

echo "== A. 基本执行 =="
run_sbx true && echo "A PASS"
echo "== B. 断网(unshare-net)=="
run_sbx bash -c 'timeout 3 bash -c "echo > /dev/tcp/1.1.1.1/443" 2>/dev/null && echo NET-REACHABLE || echo NET-BLOCKED'
echo "== C. 项目目录可写 =="
run_sbx bash -c "echo x > $PROJ/w.txt && cat $PROJ/w.txt"
echo "== D. 项目外写入拦截 =="
run_sbx bash -c "echo x > /usr/local/bin/escape.txt 2>/dev/null && echo D-FAIL-ESCAPED || echo D-PASS-BLOCKED"
echo "== E. 家目录只读 =="
run_sbx bash -c "echo x > ~/.sbx-probe 2>/dev/null && echo E-FAIL || echo E-PASS"
echo "== F. 工具链可见性 =="
run_sbx bash -c 'which git cargo node python3 | tr "\n" " "; echo'
echo "== G. NoNewPrivs =="
run_sbx grep NoNewPrivs /proc/self/status
echo "== H. suid 提权尝试 =="
run_sbx bash -c 'sudo -n true 2>&1 | head -1 || true'
echo "== I. Windows interop(env 保留)=="
run_sbx /mnt/c/Windows/System32/whoami.exe 2>&1 | head -3
echo "== J. Windows interop(--clearenv 后)=="
run_sbx --clearenv /mnt/c/Windows/System32/whoami.exe 2>&1 | head -3
echo "== K. binfmt_misc 现状 =="
ls /proc/sys/fs/binfmt_misc/ 2>/dev/null | head -5
echo "== L. tmpfs 盖 binfmt_misc 后 .exe 直执(预期:仍逃逸)=="
bwrap --unshare-net --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp --tmpfs /proc/sys/fs/binfmt_misc --bind "$PROJ" "$PROJ" /mnt/c/Windows/System32/whoami.exe 2>&1 | head -2
echo "== M. 显式调 /init 当解释器 =="
bwrap --unshare-net --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp --tmpfs /proc/sys/fs/binfmt_misc --bind "$PROJ" "$PROJ" /init /mnt/c/Windows/System32/whoami.exe 2>&1 | head -2
echo "== O. 挂全新空 binfmt_misc 实例(预期:非特权被拒)=="
bwrap --unshare-net --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp --bind "$PROJ" "$PROJ" sh -c 'mount -t binfmt_misc binfmt_misc /proc/sys/fs/binfmt_misc && ls -A /proc/sys/fs/binfmt_misc/ && /mnt/c/Windows/System32/whoami.exe' 2>&1 | head -4
echo "== P. /dev/null 盖住 /init 后 .exe(预期:收口)=="
bwrap --unshare-net --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp --bind "$PROJ" "$PROJ" --ro-bind /dev/null /init /mnt/c/Windows/System32/whoami.exe 2>&1 | head -2
echo "== R. 盖 /init + 断网 + 工具链回归 =="
bwrap --unshare-net --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp --bind "$PROJ" "$PROJ" --ro-bind /dev/null /init bash -c 'git --version && python3 --version && echo w > '"$PROJ"'/w2.txt && cat '"$PROJ"'/w2.txt && (timeout 3 bash -c "echo > /dev/tcp/1.1.1.1/443" 2>/dev/null && echo NET-REACHABLE || echo NET-BLOCKED)'
