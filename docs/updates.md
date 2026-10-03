# 应用内更新

旧版首次安装带更新器的 0.1.5，之后通过「设置 → 通用 → 软件更新」或助手/托盘右键的「检查更新…」获取新版，再点「更新并重启」。0.1.4 已有更新器，可直接检查更新。更新沿用安装位置，账号选择、DPAPI 凭据和助手设置保存在原有用户目录，安装过程不清空这些数据。

默认发布地址：[GitHub Releases](https://github.com/wapearoudy/PulseWin/releases)。应用读取 `https://github.com/wapearoudy/PulseWin/releases/latest/download/latest.json`，无需 GitHub 登录。也可在「更新来源」填入绝对本机目录，例如 `E:\AI\ai-usage\PulseWin\updates`，保存后检查。

自动检查默认开启：启动 20 秒后，以及运行中每 30 分钟检查一次。自动检查不自动执行安装；发现新版本时托盘提示更新可用，应用内显示版本和更新说明。离线、错误清单、下载失败或签名拒绝会保留当前版本，并允许重新检查。

## 发布后续版本

```powershell
npm run release
# 核对版本/更新说明后提交源码，再发布到已配置的 GitHub 仓库：
git add .
git commit -m "Release next version"
npm run publish:github
```

`npm run release` 自动递增 patch 版本，构建 NSIS/MSI、签名，并原子发布本机 `updates/latest.json`。`npm run tauri -- build` 构建当前版本，不增加版本号。已安装版本只接受更高版本；改了程序后发布必须增加版本号。

`npm run prepare:github -- wapearoudy/PulseWin` 可单独准备发布附件；`npm run publish:github` 推送已提交的 main 分支，创建草稿 Release、上传 NSIS/MSI/签名/清单，完成后发布。没有 GitHub 登录时不会自动打开认证或打印凭据；可用 Git Credential Manager 完成登录，或用环境变量 GH_TOKEN/GITHUB_TOKEN。令牌不写入源码和更新清单。

`release-channel.json` 保存发布仓库和新安装程序的默认更新来源。修改更新来源不修改签名信任。所有更新包都使用内置公钥验证，签名同时绑定真实版本，伪造清单不能将旧包宣称为更高版本。签名私钥位于当前构建用户的 `~/.pulsewin-signing/updater.key`，不进入代码仓库或 Release；必须妥善备份，丢失后无法为已安装用户继续签发更新。

本机更新使用短暂的随机 loopback 端口将所选清单和包交给 Tauri 官方签名校验/Windows 更新器，不暴露目录或凭据，不运行常驻服务。插件允许本机 HTTP 传输；应用对外更新来源与包地址只接受 HTTPS，前端没有插件直接安装权限。

## 验证范围

`npm run test:update` 用隔离 APPDATA/WebView2、独立实例身份，调用真实 Tauri 更新插件。验证签名下载、篡改包拒绝、签名版本不一致拒绝、相同版本跳过和账号偏好不变；测试不会执行安装器或修改已安装程序。实际安装过程的退出/覆盖/重启需另外验收，不能用下载校验代替。

0.1.5 统一更新配置与其他应用设置的保存目录。回归明确检查隔离目录中的 updates.json、全新配置使用默认发布来源，以及个人更新设置字节保持不变。0.1.4 的早期测试没有验证这层隔离，在线验收发现后修复。
