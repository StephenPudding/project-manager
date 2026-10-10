# Security

Project Manager executes commands from projects you choose. It is not a sandbox. Only manage trusted projects. Data stays in the selected local folder and is not encrypted by the application. Migration retains the old folder as a backup. Management uses internal Rust messages with no HTTP control API. Starting a project exposes its preview to your local network until you stop it; use a trusted network.

Report vulnerabilities using this repository's **Security → Report a vulnerability** entry. If unavailable, ask the maintainer to enable a private reporting channel without posting sensitive details. Do not submit real project data, credentials, or full caches.

The current development line is `0.1.x`. There is no published independent security audit or support-time guarantee.

管理器会执行项目命令，不提供沙箱。漏洞请通过 GitHub 私密报告渠道提交，不要公开用户数据、截图、完整缓存或凭据。
