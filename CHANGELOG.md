# Changelog

All notable changes to Kubyl are documented here.

## [0.1.0] - 2026-09-24

### Bug Fixes

- Don't watch settings.json in tests ([20ded92](https://github.com/BirknerAlex/kubyl/commit/20ded92f784a0488dc5ce5a9e650b87379854733))
- Unix-only items in shell_env broke the Windows build ([f57caab](https://github.com/BirknerAlex/kubyl/commit/f57caab8805dbf0b6aa741dc3991bdcf7b3fdf27))
- Title-bar cluster icon uses the cluster's color tag ([bdbe034](https://github.com/BirknerAlex/kubyl/commit/bdbe03444ca4e05328106210945a7664f0883268))
- Mockup column widths for pods, clippy cleanups ([4b5d7ba](https://github.com/BirknerAlex/kubyl/commit/4b5d7ba50f64e74fc4a20ad786babef5badbf78d))
- Open dialogs and switchers from actions again ([b786a31](https://github.com/BirknerAlex/kubyl/commit/b786a3105112d24b570d49832f0b551a101aa4b8))
- Format secondary- keystrokes as ⌘ / Ctrl ([0a61d1a](https://github.com/BirknerAlex/kubyl/commit/0a61d1a2cfb2ada30b17c21f37208847d5e181e3))
- Screenshot harness draws fresh frames before capturing ([4055501](https://github.com/BirknerAlex/kubyl/commit/40555010ae41c7814ef67429bf15217635b9856e))
- Defer window access, drop weak matches ([0ee86b2](https://github.com/BirknerAlex/kubyl/commit/0ee86b2d163d6121c7db5eddf2ba77620cbc1b26))
- Undo dialog uses the same PROD confirmation as delete and drain ([8bfe267](https://github.com/BirknerAlex/kubyl/commit/8bfe2671999117419f9b553e9d658ab67545b246))
- Enter confirms, ⌘↵ opens in the new split, list actions first ([4fe76bc](https://github.com/BirknerAlex/kubyl/commit/4fe76bc9bdd7a25a9353cb4c13034d829a0163a9))
- Dialog buttons closed the dialog instead of confirming ([6bcf3ed](https://github.com/BirknerAlex/kubyl/commit/6bcf3ed59fcf01006ec213e63f93fb4c8eb40710))
- Late objects, masked last-applied annotation, picker focus ([a9670be](https://github.com/BirknerAlex/kubyl/commit/a9670be851e2266ef12844d6758bc89fdd98db7d))
- Compact hover docs, one line of diff context, managedFields key ([82579af](https://github.com/BirknerAlex/kubyl/commit/82579af550d4d3452225c2c59b4c8a750b32fc2f))

### CI

- Fmt, clippy, test and build on macOS, Windows and Linux; cargo-deny ([3f012d6](https://github.com/BirknerAlex/kubyl/commit/3f012d66c6ebb29b2eedc4809edd13a6dd92f215))

### Documentation

- Record phase 00 decisions and handoff notes ([c4acbc0](https://github.com/BirknerAlex/kubyl/commit/c4acbc017ebab9831ddf50ce585f0d24556b6b3a))
- Phase 00 screenshots rendered on macOS ([7842d49](https://github.com/BirknerAlex/kubyl/commit/7842d4963e35765d7024de441e2c8fd9fba0d16d))
- Document the API for the explorer and other crates ([b12de2d](https://github.com/BirknerAlex/kubyl/commit/b12de2db6d76a864697fd3089615bf7130ad6d59))
- Phase 01 status, handoff log and decisions ([1655a57](https://github.com/BirknerAlex/kubyl/commit/1655a57a95f238a3ad0cd23b785fdc3ec0471dea))
- Phase 01 Clusters view screenshot (macOS, connected to kind) ([70c0e0d](https://github.com/BirknerAlex/kubyl/commit/70c0e0d60daf946997d72bd5144ef4010ab9099d))
- Add AGENTS.md (commands, git workflow, gotchas for coding agents) ([5a0aec5](https://github.com/BirknerAlex/kubyl/commit/5a0aec50c8e11334bda926df4eb5d597d065b67d))
- Phase 02 status, handoff log, decisions and Pods screenshot ([d762aaf](https://github.com/BirknerAlex/kubyl/commit/d762aaf18edac641a1b69b6bd48200d1e83df3fc))
- Phase 03 status, handoff log, decisions and palette screenshot ([f79fdcf](https://github.com/BirknerAlex/kubyl/commit/f79fdcf4fa436de09ac49acffaa259aa24bbf62f))
- Add phase 11 service web views and phase 12 Argo CD, web view mockup [skip ci] ([ceb8ba7](https://github.com/BirknerAlex/kubyl/commit/ceb8ba78f238480f38a017d675dd8a114bcc8836))
- Phase 04 status, handoff log, decisions and YAML screenshot ([21e57f0](https://github.com/BirknerAlex/kubyl/commit/21e57f0c40b65682c9f77ee159f5199331ee3520))
- Phase 02 follow-ups from phase 04 [skip ci] ([38f8d82](https://github.com/BirknerAlex/kubyl/commit/38f8d82cd0adef62269c6a3a541f1a5ee108023d))

### Features

- Tokio bridge, shared types, notifications and registries ([d18f980](https://github.com/BirknerAlex/kubyl/commit/d18f98030a81da530edfa24dd03bf8db7644152b))
- Typed settings.json and state.json stores ([e8a7344](https://github.com/BirknerAlex/kubyl/commit/e8a73448e0f2d8cec77efd407ea0c19c9cac6345))
- Theme tokens, bundled fonts and icons, chrome components ([69f8acc](https://github.com/BirknerAlex/kubyl/commit/69f8accae118ae0ada0cc4f979bf40cc706d8b9e))
- Zed-style workspace shell with panes, docks and persisted layout ([034bc31](https://github.com/BirknerAlex/kubyl/commit/034bc311faca22687a6d254fea22a844417dfa56))
- App icon for Windows, X11 and macOS bundles ([078314a](https://github.com/BirknerAlex/kubyl/commit/078314a56f5ce2667ba8781473fced3c22cb7bdb))
- Kubeconfig sources, settings and auth methods ([64bc464](https://github.com/BirknerAlex/kubyl/commit/64bc464eb086175f5c73b4d9251df229b98ab6b9))
- Connection manager, discovery, watches, RBAC and OpenAPI ([a6ccb4b](https://github.com/BirknerAlex/kubyl/commit/a6ccb4bba42f02b75eadc2712f6423c22b9e4cea))
- Clusters view, cluster switcher, sign-in and exec prompts ([e3769bf](https://github.com/BirknerAlex/kubyl/commit/e3769bf4942142937dafd57e2414ce5b09bf68f9))
- Wide columns, tinted cells and a FilterSidebar action ([259e104](https://github.com/BirknerAlex/kubyl/commit/259e104cf6b703ca773988ab1872fb03f8bea404))
- Watch caches, columns, filters and operations ([2690368](https://github.com/BirknerAlex/kubyl/commit/2690368bea00bf78204b5a2b1058a24d30bc8963))
- Cluster tree, favorites, resource lists and details dock ([ff72261](https://github.com/BirknerAlex/kubyl/commit/ff722612d8916f80a53fad173d95aa93901e2cac))
- Drop the sample explorer and table, window-wide kubeconfig drop ([818ec3e](https://github.com/BirknerAlex/kubyl/commit/818ec3ef07b1c7ce0a42181520e3c7854c474bf8))
- ActionRegistry::set_keystrokes for rebound actions ([9770012](https://github.com/BirknerAlex/kubyl/commit/97700128f49e005fc6b40582e4d4f6a92109ba68))
- ResourceStores::all to search loaded caches ([08835f2](https://github.com/BirknerAlex/kubyl/commit/08835f2f36be178e86ecc250b7198ffef5a45e10))
- SetFilter action, open_filtered, list commands in the registry ([c1e5030](https://github.com/BirknerAlex/kubyl/commit/c1e5030d659b603b76619090a7405f075c0b2fde))
- Per-pane back/forward, shell actions in the registry ([cadde48](https://github.com/BirknerAlex/kubyl/commit/cadde487d7480a758682a8fe964d0e44ba1dd038))
- Command palette with modes, fuzzy matching and k9s commands ([868192b](https://github.com/BirknerAlex/kubyl/commit/868192b701da3b3cac653670f7c9f73214c28c6f))
- Default and k9s presets, user bindings in settings.json ([685e392](https://github.com/BirknerAlex/kubyl/commit/685e3924b349584df0005944084e994287898da5))
- Screenshot steps keys=, wait= and action= ([c209651](https://github.com/BirknerAlex/kubyl/commit/c209651f494310e0dae31d3eac1ed55722778060))
- Rollout history and undo for StatefulSets and DaemonSets ([88e2506](https://github.com/BirknerAlex/kubyl/commit/88e2506efcb702109216c0197de7dbcee8ce357a))
- Rollout undo for StatefulSets and DaemonSets ([9e33da8](https://github.com/BirknerAlex/kubyl/commit/9e33da8dd952d1c9e17dab06664ba18dce72abd7))
- YAML model, OpenAPI schemas, validation, diff and apply ([0196314](https://github.com/BirknerAlex/kubyl/commit/01963149dcdd15d7354799089734ce91d8fb6855))
- Screenshot steps mouse= and click= ([4910c94](https://github.com/BirknerAlex/kubyl/commit/4910c9419a41c64e5b766601765849adeca134ca))
- One Dark colors for code editors ([f78bc94](https://github.com/BirknerAlex/kubyl/commit/f78bc94d1c6ba742cbbce376cd7ace35d5e2aa45))
- YAML editor view (board 3) ([8f2de8c](https://github.com/BirknerAlex/kubyl/commit/8f2de8cd9af8fc169ea0c2274b6dce458712b52e))
- Manual build/release pipeline ([4cfa19d](https://github.com/BirknerAlex/kubyl/commit/4cfa19d078f206a47f35ed58f317c557163b1a23))

### Miscellaneous

- Initial plans, mockups and logo assets ([e38451c](https://github.com/BirknerAlex/kubyl/commit/e38451c35125df9d00e1b046b77425c3a6ad1e39))
- Cargo workspace with pinned GPUI snapshot and stub crates ([8e2eba0](https://github.com/BirknerAlex/kubyl/commit/8e2eba0cc7d93456df9fe213603b7aa14d5305a8))
- License under MIT OR Apache-2.0 ([49cdd1e](https://github.com/BirknerAlex/kubyl/commit/49cdd1e8ead471fb03fe9b79f72aff23b16a7c27))
- Script/dev-cluster.sh for a local kind cluster with sample workloads ([1c2f294](https://github.com/BirknerAlex/kubyl/commit/1c2f2946b22165030bfc7821be9313d0f850e78d))

### Performance

- Cache name keys and health per object in big lists ([9aea70d](https://github.com/BirknerAlex/kubyl/commit/9aea70d4885a280c3131fcd1992e5da33b32b1fc))

### Refactor

- Make the confirmation dialogs public for other crates ([b3cdbac](https://github.com/BirknerAlex/kubyl/commit/b3cdbacc504364575adae9817442a2218d3a6c66))
- Make the reference resolver public ([f5cee68](https://github.com/BirknerAlex/kubyl/commit/f5cee68659a0c951e68a6e37c1a9df822a02ea76))

### Testing

- Live kind and OIDC tests, script/oidc-dev.sh ([3f55634](https://github.com/BirknerAlex/kubyl/commit/3f556343851743c41c6760d49b99391f5fd00af4))
- Match kubeconfig fixtures by file name (Windows paths) ([d9593bc](https://github.com/BirknerAlex/kubyl/commit/d9593bc01f58f3a695f946758f690d6cf359ff28))
- Live tests for the operations against kind ([5f51e53](https://github.com/BirknerAlex/kubyl/commit/5f51e53bb87145b4d3fe414adb59e05eb2d8b96d))
- Keymap presets only name registered actions ([2444a23](https://github.com/BirknerAlex/kubyl/commit/2444a23fb32b5f93228efe0e793935f79e389933))

### Build

- Enable kube's ring TLS backend, add auth dependencies ([1fbd931](https://github.com/BirknerAlex/kubyl/commit/1fbd9315878318fe5d7a751a2be4f27bac85c277))
- Ignore RUSTSEC-2023-0071 (rsa via openidconnect) ([805f67a](https://github.com/BirknerAlex/kubyl/commit/805f67a1b024fe0097efdcb4f2d5953fe2fd6548))
- Add http-body-util and tower's buffer feature ([e63d9e3](https://github.com/BirknerAlex/kubyl/commit/e63d9e3ece8e93c03c3297ee92680438dbfa762d))
- Add regex and serde-saphyr to the workspace dependencies ([0a5520f](https://github.com/BirknerAlex/kubyl/commit/0a5520fff3415b2234116b5d6d3f9f2cd62bdda3))
- Add nucleo-matcher (MPL-2.0) for the command palette ([756a117](https://github.com/BirknerAlex/kubyl/commit/756a117d6a15231e334db5d1a6140719ae2287ad))
- Add granit-parser, similar and lsp-types to the workspace ([c49719b](https://github.com/BirknerAlex/kubyl/commit/c49719b8fbfb3ff90fb88fdfff97d4a28ca9cea5))

### Script

- Load-pods.sh creates 5,000 pods (and churn) for list checks ([3e2a307](https://github.com/BirknerAlex/kubyl/commit/3e2a307676a22a0d17636b8dc565ee0229d0d2a0))


