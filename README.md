# XSX-Live: Xbox Series X/S 局域网原生画面串流与分发中枢

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Docker](https://img.shields.io/badge/docker-compose-blue.svg)](docker-compose.yml)
[![Rust](https://img.shields.io/badge/language-Rust-orange.svg)](receiver)

> **极低开销、零转码、2K 60fps 原画** 的 Xbox 局域网 WebRTC 接收器与流媒体中枢。
> 专为 OBS、抖音直播伴侣、局域网流媒体分发设计。

---

## 🌟 项目亮点

* **🚀 原生视频直拷贝（Stream Copy）**：直接从 Xbox WebRTC 媒体流提取 H.264 视频帧无损转封装，**不解码、不重编码、无画质损失**，完全无需 GPU/核显支持。
* **⚡ 极致轻量**：核心接收端采用 Rust 编写（基于 `str0m` WebRTC 协议栈），流媒体采用 C++ (`SRS 6.0`)。待机内存仅约 **20 MB**，满载串流内存仅 **~40 MB**，单核 CPU 占用仅 15% 左右。
* **🎛️ 现代化 Web 控制面板**：提供直观的 Web 管理界面（默认端口 `8099`），支持一键启停串流、断线重连、实时帧率/码率监控与静音画面预览。
* **🎬 丰富的播放与拉流协议**：同时支持 HTTP-FLV、RTMP、以及集成自托管 `mpegts.js` 的低延迟浏览器播放页。
* **🔒 安全与纯局域网传输**：仅在认证与会话协商阶段使用微软官方 API；音视频数据传输强制限定在局域网内，拒绝公网 STUN/TURN 中继。

---

## 📐 架构设计

```
┌────────────────────────┐
│  Xbox Series X / S     │ (192.168.x.x)
│  LAN WebRTC Server     │
└───────────┬────────────┘
            │ 局域网 UDP / SRTP (H.264 2560x1440@60fps + Opus)
            ▼
┌────────────────────────────────────────────────────────┐
│ xsx-receiver (Rust Native + FFmpeg Pipe)               │
│  ├─ WebRTC 信令与解包 (str0m)                          │
│  ├─ H.264 零转码直拷贝 (Stream Copy)                   │
│  ├─ 音频转码 (Opus -> 48kHz AAC 160kbps)               │
│  └─ Web 管理面板 (:8099) & 状态管理                   │
└───────────┬────────────────────────────────────────────┘
            │ 本地 RTMP 推流
            ▼
┌────────────────────────────────────────────────────────┐
│ xsx-media (SRS 6.0 超低延迟流媒体引擎)                 │
│  ├─ RTMP 拉流 (:1935/live/xsx)                         │
│  ├─ HTTP-FLV 拉流 (:8888/live/xsx.flv)                 │
│  └─ 内嵌播放器网页 (:8888/xsx/index.html)              │
└───────────┬────────────────────────────────────────────┘
            │ 局域网拉流
            ▼
┌────────────────────────────────────────────────────────┐
│ 客户端: OBS Studio / 抖音直播伴侣 / 局域网浏览器       │
└────────────────────────────────────────────────────────┘
```

---

## 🚀 快速上手

### 1. 克隆仓库与准备环境

确保已安装 `docker` 和 `docker compose`：

```bash
git clone https://github.com/<YOUR_USER>/xsx-live.git
cd xsx-live
```

复制配置文件并修改：

```bash
cp .env.example .env
```

编辑 `.env` 文件，填入你的 Xbox 局域网 IP 与本机服务 IP：

```ini
# Xbox Series X / S 局域网静态 IP
XBOX_IP=192.168.1.180

# 宿主机绑定 IP (局域网访问设为 0.0.0.0)
BIND_IP=0.0.0.0

# 管理后台监听端口
DASHBOARD_BIND=0.0.0.0:8099

# 客户端访问的 HTTP 流媒体基准地址
MEDIA_HTTP_BASE=http://<YOUR_HOST_IP>:8888

# RTMP 流目标地址
OUTPUT_RTMP=rtmp://127.0.0.1:1935/live/xsx
```

---

### 2. 配置 Xbox 账号认证

Xbox 远程串流需要微软账号授权。将授权凭据放置于 `auth/session.json`：

```bash
cp auth/session.json.example auth/session.json
chmod 700 auth && chmod 600 auth/session.json
```

`auth/session.json` 格式如下：

```json
{
  "client_id": "YOUR_OAUTH_CLIENT_ID",
  "refresh_token": "YOUR_REFRESH_TOKEN"
}
```

> 💡 **提示**：可使用任意标准 Xbox 客户端登录流程（如 `xbox-xcloud-player` 或 `OpenXbox` 项目的登录工具）获取有效的 `refresh_token`。接收端会在启动时自动使用该凭据续期 Token。

---

### 3. 启动服务

```bash
docker compose up -d
```

查看运行状态：

```bash
docker compose ps
docker compose logs -f receiver
```

---

## 🖥️ 管理面板与使用

打开浏览器访问管理页面：
`http://<YOUR_HOST_IP>:8099/`

* **开启串流**：连接已开机的 Xbox，接收画面并推送到流媒体分发服务。
* **停止串流**：结束当前接收会话、停止本地推流，并暂停自动重连。
* **重新连接**：快速重启会话，不重启整个服务。
* **实时监控**：可查看实时分辨率（最高 2560×1440）、实时帧率、码率及已串流时长。
* **画面预览**：按需开启低延迟静音画面预览。

---

## 📺 OBS 与直播伴侣接入

在 **抖音直播伴侣** 或 **OBS Studio** 中添加来源：

1. **方式一：浏览器来源（推荐）**
   * URL: `http://<YOUR_HOST_IP>:8888/xsx/index.html`
   * 分辨率：`1920x1080` 或 `2560x1440` (16:9)
   * 勾选“控制此来源的音频”（如需推游戏原声）
2. **方式二：网络媒体源 / HTTP-FLV**
   * URL: `http://<YOUR_HOST_IP>:8888/live/xsx.flv`
3. **方式三：RTMP 拉流**
   * URL: `rtmp://<YOUR_HOST_IP>:1935/live/xsx`

---

## ⚙️ 环境变量说明

| 变量名 | 默认值 | 描述 |
| :--- | :--- | :--- |
| `XBOX_IP` | `192.168.1.180` | Xbox 主机的局域网 IP 地址 |
| `BIND_IP` | `0.0.0.0` | SRS 端口在宿主机的监听绑定 IP |
| `DASHBOARD_BIND` | `0.0.0.0:8099` | 管理页面监听 IP 与端口 |
| `MEDIA_HTTP_BASE`| `http://127.0.0.1:8888` | 客户端访问的 HTTP-FLV/Web 播放器根路径 |
| `OUTPUT_RTMP` | `rtmp://127.0.0.1:1935/live/xsx` | 接收器推流至 SRS 的目标 RTMP 地址 |
| `XBOX_AUTH_FILE` | `/auth/session.json` | 容器内认证凭据挂载路径 |
| `STATE_DIR` | `/state` | 容器内状态与控制文件落盘目录 |

---

## 🛡️ 安全注意事项

* `auth/session.json` 包含你的私有微软凭据，**严禁上传至公开代码库或分享给他人**（`.gitignore` 已默认忽略该文件）。
* 本系统面向安全可信的局域网设计，Web 管理面板与拉流端口未设置复杂的公网登录密码，**请勿将 8099、8888、1935 端口直接映射至公网**。

---

## 📄 License

本项目基于 [MIT License](LICENSE) 开源。协议逆向与信令实现参考了社区优秀的开源项目（如 `DRHATL95/kite` 与 `unknownskl/xbox-xcloud-player`），在此深表感谢。
