# 桌面助手外观与交互验收

参考为用户提供的七项黑色悬浮助手截图，以及 `Pulse-original/Sources/Pulse/Panel` 的绘制和布局代码。截图里的桌面壁纸不属于助手窗口；未知截图缩放比例不能作为 Windows 像素宽度的标准。以下是源码尺寸和实际界面的验证，不是“100% 还原”声明。

| 对照项 | 本次实际结果 | 证据与边界 |
| --- | --- | --- |
| 七项自由悬浮胶囊 | 64 CSS px × 630 CSS px，88 px 间距；黑色表面、圆形端点 | `e2e/assistant.spec.ts` 与 `screenshots/assistant-capsule.png`；已目视检查，无外扩路径或透明断裂 |
| 第一圆环与人物 | 圆环布局 36 px，中心距胶囊顶部 40 px；机器人画布 28 px | SVG 资源直接采用原版；机器人状态和随机时序使截图帧不同，不计算虚假的像素还原率 |
| 详情卡 | 250 px 内容宽、20 px 尾部预算、8 px 间隔；卡片打开不改原生窗口预算 | `assistant-detail-card.png`；错误消息及连接动作保持在卡片内 |
| 只有真实余额 | 短金额在助手上、完整币种/金额在卡片与设置；没有额度时不画百分比 | 浏览器完整流程验证报告的 0 也显示为金额；旧读数有日期标记 |
| 剩余模式与内环 | 未知额度不反转成满环；内环优先同一模型池，跟随账号色 | 通过真实 React/SVG 流程验证，模型池缺省时保持缺省，不从显示标签猜分组 |
| 收起与鼠标命中 | 320 ms 离开延迟，浮动始终展开；实际轮廓与悬停宽容带分离 | SVG 原路径展平后交给 Rust；几何与协议测试通过，仍需原生下层窗口和快速边界点击验收 |
| 动画 | 原版弹簧、眼睛、身体变形、全部粒子效果、随机手势与 6.4 秒重置庆祝 | 时间和事件证据有测试；减少动态效果为稳定帧；不能用单帧截图证明整个动画完全一致 |
| 同屏位置恢复 | 原生验收发现空条变七项时 Y 从 80 漂到 45，修复后保持 [720,80] | 0.1.2 原生发布版通过真实 WebView2/IPC 验收，DPI 1.5；见 `screenshots/native-assistant.png` |

设置与新功能截图保存在 `screenshots/`，还包括 `settings-appearance.png`、`application-general.png`、`application-shortcut-conflict.png`、`token-spend-overview.png` 与 `native-settings.png`。这些使用隔离 fixture，不包含真实账号结果。WebView 截图不包含桌面合成效果，不能代替真实 Windows 桌面上的玻璃或点击穿透验收。

0.1.2 的 Computer Use 应用授权超时。0.1.3 重新验收时可读取隔离测试版原生窗口，实际发送 Control+Alt+Shift+F12 后设置窗口正常打开。背景拖动被工具目标窗口检查拦截，报落点属于 explorer.exe FolderView，激活与重截后仍被拦截；这项没有计为通过。

尚未完成的外观/体验项目：Windows 玻璃材质、完整语言/数字本地化、全部详细卡来源及 prompt cache 到期、账号拆分环、所有认证与恢复流程；真实多屏、混合 DPI、虚拟桌面、游戏全屏和原生拖动的验收也未全部完成。完整功能边界见 `readme-coverage.md`。

0.1.3 详细卡：已检查 screenshots/assistant-detailed-card.png。原版 250px 主体、20px 尾部，原版详细内容高度预算；今日/7日/31日、31根日柱、主模型和缓存命中率均使用对应来源的归一化记录。估算金额明确标识为 API 价格估算。记录缺失显示缺失状态，未启用读取不发出 history IPC。

修复可复现的旧仪表盘 CSS 泄漏：卡片11px内边距使 SVG 背景与内容不对齐、底部圆角被裁切，同时旧 ring 样式覆盖助手轨道透明度。旧组件采用独立 provider-card/provider-ring 类名，真实浏览器检查零内边距/零边框/透明容器与固定尾部预算通过。截图目视确认边角连续，没有额外灰边和计划标签装饰。

0.1.4 更新设置：screenshots/native-updates.png 为真实发布版 WebView2、隔离用户目录。通用设置显示当前版本、来源、自动检查开关和手动检查入口。来源编辑和下载/安装状态有浏览器回归；签名拒绝由原生插件验证。截图不作为安装器实际覆盖/重启的证明。

0.1.6 侧栏一致性：此前通知、Token 消耗、通用缺少图标列，文字比其他入口偏左。7 个设置入口改为共用渲染结构，图标均采用单色 SVG；新增三项对应原版 bell/chart.bar/slider.horizontal.3，外观、圆环、位置也对应原版图标语义。图标列均为18px、间距10px；960px 窗口下文字左边界均为50px、行高均为33.5px。真实浏览器检查浅色/深色的图标颜色跟随文本和选中状态，720×480 无横向溢出，三项导航和搜索正常。截图：screenshots/sidebar-consistent-light.png、screenshots/sidebar-consistent-dark.png。

0.1.7 滚动条：设置中的垂直/水平滚动区域统一为8px宽度、4px可见圆角滑块，轨道/角落透明，去掉上下箭头。侧栏和内容区固定窄滚动条空间，避免筛选或页面切换改变内容宽度；明暗配色、悬停、拖动和系统高对比度均使用对应颜色。浏览器专项检查明确禁用 Playwright 默认的 --hide-scrollbars 参数，以真实绘制和命中滚动条；侧栏及内容区滚轮、实际滑块拖动、720×480 布局均通过。截图使用隔离模拟账号：screenshots/scrollbars-light.png、screenshots/scrollbars-dark.png。

0.1.7 发布版 WebView2 实测两处滚动条8px宽、透明轨道、无箭头；截图 screenshots/native-scrollbars.png 为隔离 APPDATA 的原生窗口，未启用账号采集。35项浏览器流程、原生助手、CLI、签名更新验收均通过；发布包哈希及检查边界在 release-0.1.7.json。
