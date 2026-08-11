#!/usr/bin/env node
// scripts/remote-e2e-smoke.mjs — remote daemon 端到端冒烟(implement.md Step 9)
//
// 模拟 PC daemon(Node 22 内置 WebSocket)+ 手机(fetch),验证完整链路:
//   1. WSS 连接(shared_secret 握手)
//   2. 配对码生成(internal RPC,经 WSS)→ 6 位码
//   3. 手机 redeem → device_token
//   4. 手机 nodes → 节点在线状态
//   5. 手机 proxy 请求 → remote 转发 Request 帧 → PC 回 Response 帧 → 手机拿响应
//
// 用法:
//   cargo build -p everlasting-remote --release
//   ./target/release/everlasting-remote --port 7457 --shared-secret test123 &
//   node scripts/remote-e2e-smoke.mjs
//
// 依赖:Node >= 22(内置 WebSocket)。端口/secret 与脚本头常量一致。
const REMOTE = 'http://localhost:7457';
const WS_URL =
  'ws://localhost:7457/ws?secret=test123&node_id=company-pc&display_name=%E5%85%AC%E5%8F%B8PC';

const ws = new WebSocket(WS_URL);
const pending = new Map();
let nextId = 1;
let onRequestFrame = null;

ws.onmessage = (ev) => {
  const frame = JSON.parse(ev.data);
  if (frame.kind === 'response') {
    const p = pending.get(frame.id);
    if (p) {
      pending.delete(frame.id);
      p.resolve(frame);
    }
  } else if (frame.kind === 'request') {
    if (onRequestFrame) onRequestFrame(frame);
  } else {
    console.log('[PC] unexpected frame kind:', frame.kind);
  }
};

function rpc(path) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ kind: 'request', id, method: 'POST', path, headers: [], body: [] }));
    setTimeout(() => {
      if (pending.delete(id)) reject(new Error(`rpc ${path} timeout`));
    }, 5000);
  });
}

await new Promise((res, rej) => {
  ws.onopen = res;
  ws.onerror = rej;
});
console.log('[PC] WSS connected (secret ok)');

// --- 1. 配对码生成 ---
const gen = await rpc('/internal/pairing/generate');
const { code, expires_in } = JSON.parse(Buffer.from(gen.body).toString());
if (!/^\d{6}$/.test(code) || expires_in !== 60) throw new Error(`bad code: ${code} / ${expires_in}`);
console.log('[PC] pairing code:', code, 'expires_in:', expires_in);

// --- 2. 手机 redeem ---
const redeemRes = await fetch(`${REMOTE}/api/v1/pairing/redeem`, {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ code, device_name: 'e2e 手机' }),
});
const redeemed = await redeemRes.json();
if (redeemRes.status !== 200 || redeemed.nodeId !== 'company-pc')
  throw new Error(`redeem failed: ${redeemRes.status} ${JSON.stringify(redeemed)}`);
console.log('[phone] redeem:', redeemRes.status, 'node:', redeemed.nodeId, '|', redeemed.nodeDisplayName);
const token = redeemed.deviceToken;

// --- 3. 节点状态 ---
const nodesRes = await fetch(`${REMOTE}/api/v1/nodes`, {
  headers: { authorization: `Bearer ${token}` },
});
const nodes = await nodesRes.json();
if (nodesRes.status !== 200 || nodes[0].status !== 'online' || nodes[0].displayName !== '公司PC')
  throw new Error(`nodes bad: ${JSON.stringify(nodes)}`);
console.log('[phone] nodes:', nodesRes.status, JSON.stringify(nodes));

// --- 4. 反向代理转发(非流式)---
const healthPromise = fetch(`${REMOTE}/api/v1/proxy/api/v1/health`, {
  headers: { authorization: `Bearer ${token}` },
});
const reqFrame = await new Promise((res, rej) => {
  onRequestFrame = res;
  setTimeout(() => rej(new Error('no Request frame from remote')), 5000);
});
if (reqFrame.path !== '/api/v1/health' || reqFrame.method !== 'GET')
  throw new Error(`bad frame: ${JSON.stringify(reqFrame)}`);
if (reqFrame.headers.some(([k]) => k.toLowerCase() === 'authorization'))
  throw new Error('Authorization leaked to PC!');
console.log('[PC] got Request frame:', reqFrame.method, reqFrame.path, '(auth stripped ok)');
ws.send(
  JSON.stringify({
    kind: 'response',
    id: reqFrame.id,
    status: 200,
    headers: [['content-type', 'application/json']],
    // 协议 body 是字节数组(Vec<u8> 序列化);字符串会被 remote 丢弃
    body: Array.from(Buffer.from(JSON.stringify({ ok: true, from_pc: true }))),
  }),
);
const healthRes = await healthPromise;
const healthBody = await healthRes.text();
if (healthRes.status !== 200 || !healthBody.includes('from_pc'))
  throw new Error(`proxy reply bad: ${healthRes.status} ${healthBody}`);
console.log('[phone] proxy health:', healthRes.status, healthBody);

ws.close();
console.log('E2E OK — 全链路通过');
