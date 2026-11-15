# Yoru RT-01 — ヨルシカ专属桌面播放器

基于 **Tauri v2 + TypeScript** 构建的无边框全透明新拟物桌面音乐物件，专为乐队 **ヨルシカ (Yorushika)** 设计。机身复刻浅枫木圆角框 + 哑光粉白面板 + 4:3 微凸暗色玻璃晶体显像管（CRT）与全套麦克风实录机械拟音。

## 工程架构（Deep Modules）

整个工程遵循 *A Philosophy of Software Design* 的 **Deep Module（窄接口、深实现、零泄漏）** 原则组织，前后端共拆分为 8 个正交深模块：

```text
yoruplayer/
├── public/library/            # 内置双专辑测试曲库（封面、16 首真实音频、LRC 与包络清单）
├── src/
│   ├── main.ts                # 极薄组合根（Composition Root），仅做模块装配
│   ├── core/
│   │   ├── foley.ts           # [Deep Module 1] 实录机械拟音引擎（44.1kHz PCM 预解码、箱体滤波、双阶段微动）
│   │   ├── desktop.ts         # [Deep Module 2] 桌面壳桥接（无边框拖拽、右键原生菜单、托盘事件与 asset:// 映射）
│   │   └── player.ts          # [Deep Module 3] 曲库与播放状态机（LRC 对齐、拖拽导入、macOS MediaSession）
│   ├── hardware/
│   │   ├── controls.ts        # [Deep Module 4] 物理运动学原语（极坐标 atan2 旋钮解算、按压与旋转消歧、细齿刻度）
│   │   └── crt.ts             # [Deep Module 5] 晶体显像管与零空闲 CPU 示波器（空闲/遮挡自动挂起 RAF 帧循环）
│   ├── assets/foley-samples.ts# 实录机械开关与编码器音频采样数据
│   └── styles/                # 物理机身 (chassis.css) 与显像管光学层 (crt.css)
└── src-tauri/src/
    ├── window_stage.rs        # [Rust Deep Module A] macOS NSWindow 透明异形窗、置顶/壁纸层切换与尺寸缩放
    ├── library.rs             # [Rust Deep Module B] 本地 ~/Music/Yorushika 无损专辑与 LRC 自动扫描器
    └── tray.rs                # [Rust Deep Module C] macOS 状态栏单色模板图标与机身右键原生菜单
```

## 交互与摆放模式

- **挪动机身**：按住外圈**浅枫木边框**或左上**扬声器网孔区**即可任意拖动。
- **右键机身 / 点击顶部状态栏图标**：
  - **置顶把玩模式**：悬浮于普通窗口上方，跨桌面空间跟随。
  - **桌面摆件模式**：通过 CoreGraphics `kCGDesktopIconWindowLevel` 钉在壁纸图标层之上、应用窗口之下。
  - **袖珍机身尺寸 (82%)**：一键切换角落紧凑比例。
- **本地无损曲库扩展**：将完整专辑文件夹（含 `.flac` / `.m4a` / `.mp3`、同名 `.lrc` 与 `cover.jpg`）放入 `~/Music/Yorushika/<专辑名>/`，启动或点击菜单「重新扫描」即可自动载入；也可直接把音频/歌词文件拖入机身即刻播放。

## 开发与打包

```bash
# 仅前端开发预览
bun run dev

# 启动原生无边框透明桌面应用（开发模式）
bun run tauri:dev

# 构建 macOS 原生 .app 与 .dmg 安装包
bun run tauri:build
```
