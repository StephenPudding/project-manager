# Contributing

Use an issue to describe a bug or discuss a substantial feature before starting work. Small fixes and documentation improvements are welcome.

Follow the README to build. Format Rust changes with `cargo fmt`, and describe the behavior and any build or validation results in your pull request. Say explicitly when tests were not run. Existing tests are opt-in and can launch temporary servers and WebView2; they do not run as part of the normal build.

Keep slow work outside the UI thread, preserve cached browsing without an idle worker, and never terminate processes the manager does not own. Update both READMEs when build instructions change.

Never include private project names, screenshots, caches, logs, credentials, or machine-specific paths. Use synthetic examples. Contributions are covered by the MIT license.

欢迎提交小范围修复和文档改进。大功能先开 Issue 讨论。PR 请如实说明构建和验证情况，不要将构建成功描述为测试通过。禁止上传私人项目截图、缓存、日志、凭据和路径。
