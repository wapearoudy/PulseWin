# Pulse 中文 README 功能与 UI 对照

验收来源：[官方 README.zh-CN.md](https://github.com/qunqin24/Pulse/blob/main/README.zh-CN.md)，2026-10-02 阅读在线版本；实现依据为同目录 `Pulse-original` 的 Swift 源码和 Docs。README 中的截图是外观参考，具体默认值和交互以对应源码与专题文档为准。上游当前有 77 个配额服务商，Token 消耗的 54 种客户端来源是另一套功能，不能混算。

状态含义：**已接通**表示具有运行代码及对应验证，不等于全平台验证；**部分**明确列出剩余项；**缺失**没有可供用户完成该任务的完整实现。此表不计算虚假的整体完成百分比。

| 官方能力 | 对应原版源码 / 文档 | Windows 当前状态 / 验收缺口 |
| --- | --- | --- |
| 圆环、阈值颜色与账号高亮色 | Panel/UsageRingView、UsageTint；Docs/ui/rings-and-surface.md | 已接通使用量、剩余量、账号高亮、明确用尽标记；没有百分比时保持空轨道，内外环使用同一账号颜色，用尽覆盖自定义颜色；随机动画及系统渲染差异不作为像素一致证明 |
| 数字显示规则 | Usage/ProviderUsage.figure | 已接通两端保护：有使用至少 1%，未用尽最多 99%；剩余量独立计算 |
| 工作状态旋转光点 | Usage 活动监视器；Docs/refresh-and-data.md | 已接通独立 2 秒生命周期扫描：Claude Code、Codex、Kiro、ZCode（按配置归属 z.ai/智谱）；工具/模型超时分别处理，取消选择或隐藏不触发完成；实际账号日志与休眠唤醒场景尚未验收 |
| 单账号刷新 | Docs/ui/input.md | 已接通圆环点击、650ms 最短可见时长、后台刷新事件、其他账号不跟转 |
| 第二额度内环 | Panel/UsageRingView | 已接通，按原版默认关闭；优先选择同模型池的第二额度，单一窗口不画空副环；模型池元数据仅已接入能确认来源的 adapter，不以标签猜测分组 |
| 时间窗口外弧 | Panel/UsageRingView | 已接通已过去/剩余方向；只使用服务报告的时长，其他提供商时长覆盖仍需审计 |
| 悬停详情卡 / 所有配额 / 倒计时 | Docs/ui/panel-geometry.md | 部分：原版形状、图标、黑色表面、圆角填充修复、尾部预算、悬停固定窗口、空读数错误卡预算已接通；按账号开启详细卡可显示更新时间、今日/7日/31日消费、31日图、主模型和缓存命中率，证据充分时估算窗口金额；限于已有 reader 关联的账号，prompt cache 到期和其余来源尚未覆盖 |
| 配额置顶 / 自动选取 | Usage 展示规则 | 部分：自动选择最满额度，有真实窗口 ID 时按 ID 置顶，兼容旧标签；Claude Code/Codex/Antigravity 已保留可确认的窗口身份与模型池；其他 adapter 的跨路由稳定 ID、账号拆分环仍未全覆盖 |
| 消耗速度 / 耗尽预测 | Usage；Docs/refresh-and-data.md | 已接通原版单次读数算法（不是历史斜率），默认关闭；需明确时长、重置和至少 3% 已过窗口，时间仅报告两小时以内的粗略估算 |
| 左 / 右 / 顶部 / 自由浮动 | Panel/PanelPlacement；Docs/ui/panel-geometry.md | 部分：移动中吸边、顶边转向、浮动与停靠长度变化的抓取补偿、设置切换与位置保存已接通；跨 DPI 实机行为未验收 |
| 多屏记忆 / 热插拔 / 跟随指针所在屏 | Panel/PanelPlacement、ActiveDisplayFollower | 部分：使用 Windows 显示器设备身份保存每屏位置、方向与停靠，保留断开屏记录，拔出后回落；同屏条目数量变化保留绝对位置，换屏或工作区/DPI 变化才按比例恢复；真实多屏、热插拔与混合 DPI 尚未验收 |
| 收起细线与额度预警 | Panel/DockBerthShape；Docs/ui/input.md | 部分：贴边 320ms 收起、浮动保持展开、收起预警色已接通；原生点击命中复用实际 SVG 轮廓，悬停宽容区域保持原版但不接管透明区点击；真实下层窗口穿透、快速跨边界点击和采样竞态尚未完整验收 |
| 系统通知 | Usage/UsageAlerts；Docs/notifications.md | 部分：默认关闭；阈值、用尽、明确重置、连续三次故障、真实货币余额阈值及持久去重已接通，旧缓存不证明重置，30 分钟新缓存保护故障通知；Windows 实际横幅、点击进入对应账号、跨路由稳定窗口身份尚未验收或补齐 |
| 全屏避让与工作区行为 | Panel/FloatingPanel；Docs/ui/panel-geometry.md | 部分：默认开启 Windows 前台全屏检测、同屏避让和恢复；排除桌面、自身窗口与普通最大化窗口；实际游戏/视频/虚拟桌面尚未验收 |
| 黑色 / Liquid Glass 材质 | Panel/PanelSurface；Docs/ui/rings-and-surface.md | 黑色已接通；Windows 玻璃材质仍缺，不以半透明 CSS 宣称等价 |
| 按账号机器人开关 | Panel/BotMark；Settings 账号外观 | 已接通，默认供应商图标，按账号启用机器人 |
| 八种人格 / 十八种形状 / 自定义颜色 | BotMarkPersona、Body、Choreography | 已接通设置与动画；原版完整编排表直接导入，原始身体/眼睛资源原样使用；人格与身体独立 |
| 机器人完整动作 | BotMarkEngine、Morphs、Particles、Programme | 部分：原版持久弹簧、表情序列、注视、全部任务形态、前后粒子带、随机手势时序、形态连续性与 6.4 秒重置庆祝已移植，完成动作只由见证过的工作结束触发；默认不重播旧重置，减少动态效果使用稳定静态帧；公式和编排有测试，随机动画不声称逐帧图像一致 |
| 右键悬浮栏 / 收起细线菜单 | AppDelegate.panelMenu；Docs/ui/input.md | 已接通原生设置/退出菜单、菜单打开期间保留展开；检查更新菜单和通用设置更新入口已接通；真实安装覆盖/重启仍需单独验收 |
| 可配置全局快捷键 | App/GlobalShortcut；Docs/ui/input.md | 部分：打开设置和显隐助手两个快捷键默认未设置；录入、取消、删除、系统注册和逐项冲突提示已接通；真实 OS 按键响应单独记录，浏览器录入测试不算全局响应验收 |
| 五种语言 | Resources/*.lproj；Docs/development.md | 部分：设置中文、供应商原名；面板统一翻译、其他四语及大数规则尚缺 |
| 同服务多账号 / 自定义标签 | AccountKey；Docs/providers/README.md | 缺失完整账户隔离与附加登录流程，不能把 77 个供应商当成多账号 |
| Token 消耗本机读取开关 / 54 来源 | Docs/token-spend.md | 部分：默认关闭，11/54 实际 reader：Claude Code、Codex、Qwen Code、Gemini CLI，OpenCode/Kilo CLI/MiMo Code 的只读 SQLite，以及 Cursor、Antigravity、Hindsight、MCode 的导出/同步/捕获文件；后四项需要用户先准备文件，不会自动同步；目录扫描、流式解析、取消、去重、缓存与坏 metadata 有测试；SQLite 的 WAL 更新失效、只读、不改写数据库、限时、取消和镜像去重有 fixture 测试；新增数据库 reader 尚未用真实用户日志验收；其余 43 来源含 Antigravity IDE 原生数据库未覆盖 |
| 7 天区间 / 公开价格估算 | Docs/token-spend.md | 部分：今天/7 天/30 天、公开 models.dev 价格、24 小时价格缓存与离线旧价格已接通；未知模型和未分类 Token 不伪造费用；Cursor 日聚合导出不编造小时，相关范围的小时图明确不可用；真实用户日志及在线价格刷新尚未验收 |
| 模型 / Agent 明细 / 日与小时图 / 排序分页 | Settings TokenSpend；Docs/token-spend.md | 已接通模型详情、Agent、日/小时图、项目与会话排序分页、重扫/停止；离开页面停止未完成扫描，已完成快照可重用，关闭设置释放会话；使用 fixture 验证 UI，来源数量仍受上一行限制 |
| 77 个额度服务商与各自登录来源 | Docs/providers/README.md、Docs/setup | 部分：注册 77 个适配器，不代表路由、登录回退、账户隔离或真实账号已验收；逐提供商检查另列 |
| 缓存、连接诊断、重连与脱敏报告 | Docs/refresh-and-data.md；ConnectionRemedy | 部分：最后有效读数与采集时间持久化，24 小时缓存上限、过期窗口剔除、失败时旧读数标记、旧结果防回滚已接通；来源元数据仍依赖 adapter，完整回退轨迹、重连与脱敏报告尚缺 |
| `--json` 只读缓存 | Usage/UsageReport；Docs/json-output.md | 部分：启动 GUI 前只读缓存、不请求服务、不读取或写入凭据，输出启用账号的顺序/置顶额度、观察时间、年龄、真实余额与窗口；原版完整窗口身份、scope、估算、设置链接等 JSON 字段尚未补齐 |
| 开发者集成 / 账号链接 | Docs/integrations.md | 部分：只读 `--json` 和 `--statusline` 已接通；原版完整集成协议与账号深链缺失 |
| 用户扩展 | Docs/extensions.md | 缺失清单、禁用默认、运行时限、输出校验与账号呈现 |
| 首次选择 / 升级提示 | Docs/architecture.md | 部分：首次全不选、完成后才采集、禁用不请求已接通；升级新来源一次性建议未覆盖 |
| 系统代理 / 手动代理 / 自适应刷新 | Docs/networking.md、refresh-and-data.md | 部分：系统代理、手动间隔、按服务的 2/5/15/30 分钟单次调度、真实 Windows 节电/低电量约束、系统恢复通知及时间跳变到期重查已接通；活动/查看/额度变化更新节奏，外部消费最多 5 分钟，隐藏或节电 30 分钟，频繁活动不推迟到期请求；手动代理、辅助进程代理、热限制和独立显示器休眠尚缺，真实休眠恢复未操控验收 |
| 手动凭据本地加密 | Auth；README 隐私章节 | 部分：Windows 当前用户 DPAPI 加密完整凭据文档，旧 JSON 读取后原子迁移，保留 Cookie/站点等字段；损坏或不可解密文件不覆盖、不降级明文，设置显示错误；同用户往返与损坏文件有测试，跨 Windows 用户场景尚未实测 |
| 安装、开机启动、更新、托盘行为 | App；Docs/releasing.md | 部分：NSIS/MSI、单实例、托盘、实际登录启动状态与设置已接通；第二次启动打开设置；打包后修复本次产物的 Medium 完整性标签以避免 NSIS 临时文件报错；签名更新器支持本机发布目录与 GitHub Releases，启动/定期检查、版本提示、下载进度和更新重启入口已接通；签名绑定版本，拒绝降级；登录启动写入与真实覆盖重启还需验收 |

## 当前验证边界

验证截至 2026-10-03：824 项 Rust 完整回归、181 项前端单元测试、30 项浏览器流程通过。默认忽略的 OpenCode Go 只读线上测试另行执行通过，结果是订阅权限错误，未取得成功的线上额度读数。浏览器测试覆盖实际 React 界面、账号外观、动画时序/重置、固定悬停窗口、圆环布局、通知设置、Token 消耗明细、真实余额-only 展示、模型池配对及快捷键录入/冲突。原生 smoke 使用独立实例身份、隔离 APPDATA / WebView2 目录和测试事件，不改动已安装实例或真实账号；0.1.3 发布版通过真实 IPC、偏好持久化、空选择不采集、DPI 1.5 下空条变七项仍恢复 [720,80]、胶囊/圆环尺寸及自由悬浮保持展开验收。系统登录启动状态只读查询与临时快捷键系统注册通过；本版 Computer Use 实际发送 Control+Alt+Shift+F12 后，隔离测试版设置窗口正常打开。测试读数不声称是真实账号结果。

发布版只读 CLI 验收通过：启用账号筛选、真实窗口 ID 置顶、观察年龄、与面板一致的百分比显示，缓存/偏好/假凭据文件保持字节不变。可用 `npm run test:cli` 重复，使用隔离目录。NSIS/MSI 发布构建成功，三个生成文件的 Medium 完整性标签已核验；本版安装向导只做隐藏启动，未执行安装，未取得当前欢迎页截图，不将它计作安装流程实测。

真实 OS 右键菜单、原生详情卡填充和收起细线在前序版本已目视检查。0.1.3 的实际全局按键响应通过；拖动输入被 Computer Use 的目标窗口检查拦截，报告该点属于下层 explorer.exe FolderView，激活并重新截图后重试仍被拦截，因此点击穿透/拖动不计为通过。已安装实例未被操作。当前版本的多屏/混合 DPI、真实服务登录、实际通知横幅、开机启动写入均未完成实机验收；这些边界不被大量 fixture 测试覆盖。稳定截图保存到 `docs/screenshots/`，避免后续测试清理 `test-results` 后丢失证据。

## 资产和移植方式

- `src/panel/icons/*.svg`：复制自原版 Resources；不以首字母或临时图形替代。
- `src/panel/botmark/bot-data.json`：原版形状、眼睛、状态资源。
- `src/panel/botmark/choreography.json`：按原版 Swift 编排表提取全部状态顺序与停留区间。
- 人格、身体、标记颜色、额度颜色分别存储在对应账号下。
- Apache-2.0 许可证与移植说明保留在 `licenses/`。

## 0.1.3 OpenCode Go 实测与详细卡

本机 OpenCode CLI 登录文件能提供 API key；对官方 Go usage 接口只读 GET 返回 HTTP 403 / EntitlementError / OpenCode Go subscription required。新版不再把此错误等同于未登录或 API key 失效：错误卡给出订阅账户/工作区提示，连接按钮打开对应账号设置，显示实际凭据来源。环境变量、PulseWin 保存的 key、CLI 登录按此优先级读取；XDG_DATA_HOME 仅接受绝对目录。手动 key 仍使用 DPAPI 保存，不在诊断或测试输出中打印。需使用已订阅账户/工作区的 key 才能验证成功读数。

接口依据：[官方 Go usage 路由](https://github.com/anomalyco/opencode/blob/dev/packages/console/app/src/routes/zen/go/v1/usage.ts)、[Go 文档](https://opencode.ai/docs/go/)。rolling/weekly/monthly 保留服务返回百分比、稳定窗口身份、时长与限制状态；无成功数据时不编造 0%。

详细卡默认关闭，Token 本机读取亦默认关闭。两个开关都开启后才读取对应来源，关闭会清空前端历史缓存并取消进行中的读取。金额为公开 API 价格估算；缺失价格/时间戳、聚合或部分记录不用于窗口预算推算。每个账号缓存独立，切换账号不闪现前一账号数据；预留最多六项额度的窗口预算，不因悬停详情改变窗口尺寸。

视觉检查发现全局旧仪表盘 .card/.ring 样式污染助手，造成多余 11px 内边距、边框、圆角裁切和错误轨道颜色。旧组件类名隔离后，新增真实浏览器样式断言通过；截图保存在 screenshots/assistant-detailed-card.png。
0.1.3 NSIS/MSI 及应用构建成功，Medium 完整性标签核验通过。原生 WebView2 和 CLI smoke 全部通过，真实 OS 快捷键打开设置通过。安装器仅隐藏启动后终止本次测试进程，未执行安装，未取得欢迎页截图。构建文件 SHA-256 和验收边界保存于 release-0.1.3.json。
## 0.1.4 签名更新与 GitHub 发布

更新不再只有手动安装入口：设置通用页、托盘和助手右键均可检查更新，自动检查在启动20秒后和每30分钟运行一次，发现新版由用户点击更新并重启。默认来源为公开仓库 wapearoudy/PulseWin 的 GitHub Releases，也支持本机绝对更新目录。每次构建生成已签名 NSIS/MSI，并原子更新本机发布清单；发布脚本推送已提交源码、上传附件并发布 GitHub Release。私钥在用户目录，未提交或上传。

使用 Tauri 官方更新插件与 Windows 安装器，强制包签名和签名版本一致性，固定安装位置，用户目录偏好/凭据不清空。插件的普通 HTTP 能力仅用于后端短暂 loopback 传输，应用对外来源只接受 HTTPS，前端不拥有 updater 插件直接安装权限。验收与发布方法见 updates.md；真实安装覆盖与重启的验收边界单独记录。

本次完整离线回归：832项 Rust 通过（另1项在线测试默认忽略）、181项前端、35项浏览器流程通过；新增浏览器流程覆盖无新版、下载/重启状态、签名失败重试、来源切换和非法地址拒绝。
0.1.4 发布版的原生、CLI 和更新专项验收均通过。真实 Tauri 插件成功下载并验证签名；篡改字节、伪造高版本号都被拒绝，相同版本不更新，账号偏好保持不变。测试没有执行安装器，因此实际覆盖安装/重启仍不计为已验收。证据：screenshots/native-updates.png、release-0.1.4.json。

## 0.1.5 更新来源配置目录修复

0.1.4 在线验收发现更新配置使用 Windows Known Folder，未跟随应用的 APPDATA 设置目录，隔离配置会继承旧来源。0.1.5 改为使用与助手/账号设置相同的目录，并检查全新来源、隔离 updates.json 和个人更新设置字节不变。0.1.4 的签名下载结果仍有效，配置隔离不计为通过。最新验收及哈希见 release-0.1.5.json。
