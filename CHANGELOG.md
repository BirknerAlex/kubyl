# Changelog

All notable changes to Kubyl are documented here.

## [0.1.2] - 2026-09-25

### AGENTS.md

- Rewrap the screenshot steps ([59e4a0e](https://github.com/BirknerAlex/kubyl/commit/59e4a0e8941344bda54737a2ca622ae4ffbeeca7))

### Shared

- Harness drag and file-drop steps, dialog Enter fix, board 9 mounts ([debf90e](https://github.com/BirknerAlex/kubyl/commit/debf90e6081361671e0f626fba28fdfaa02e3dc2))
- ForwardPort/StopForward actions and the ActiveForwards global ([9354d85](https://github.com/BirknerAlex/kubyl/commit/9354d8570fe5b72ac6a1ab0a537e2a738995d422))
- Scroll step for the screenshot harness ([a8c5f47](https://github.com/BirknerAlex/kubyl/commit/a8c5f47792f9f2451e37a43c6dfd0f9283140e7d))

### Workspace

- Activating a dock panel un-zooms the pane ([c6b32ae](https://github.com/BirknerAlex/kubyl/commit/c6b32aefb619c322289407e84dc044f3dc1c9132))
- Wrap long messages in the notification history ([2113775](https://github.com/BirknerAlex/kubyl/commit/21137757c1e818cd8817e26b27ad6d53ec80de06))

### Kubyl_charts

- Line, area and stacked charts, sparklines, time ranges ([8033f0d](https://github.com/BirknerAlex/kubyl/commit/8033f0d601231898c8604a454b6f5a2b5e735228))
- Binary value axes for byte charts ([c842f9b](https://github.com/BirknerAlex/kubyl/commit/c842f9b4d00659507253310cd036789f2e765c2c))
- Fix the binary axis test expectation ([0bbdc14](https://github.com/BirknerAlex/kubyl/commit/0bbdc1456664c187784e58b83ccb175d8457e80c))
- 8 series colors, ColorRegistry, compact charts ([6b82231](https://github.com/BirknerAlex/kubyl/commit/6b822314eb6d5b7a9d1a7d16f74502c6ada1bf99))

### Kubyl_core

- DetailsSection extension point ([ba08c2f](https://github.com/BirknerAlex/kubyl/commit/ba08c2fc83d4bce90ad77aafe288d66cd84d3974))

### Kubyl_explorer

- Files sub-tab, ports with one-click forwards, scale buttons ([ff667ed](https://github.com/BirknerAlex/kubyl/commit/ff667edf6f8acca5093c73c4a0a4af1409975e63))
- Usage sparklines, metrics re-sort, Events and namespace overview routes ([b7394d3](https://github.com/BirknerAlex/kubyl/commit/b7394d324adb869bfcdfdc7a747c9ae468161899))
- Render contributed details sections ([6cd6e22](https://github.com/BirknerAlex/kubyl/commit/6cd6e22ce9a8e4027bd5a67e39db08265ff62953))

### Kubyl_files

- Remote listing, capability probe, transfers and queue ([2a6a693](https://github.com/BirknerAlex/kubyl/commit/2a6a69358100b611a4eaee6a84ae0e61fa76c4a2))
- File browser view, drag and drop, preview and edit in place ([18d2400](https://github.com/BirknerAlex/kubyl/commit/18d24006258d54eb0eecdd0e4ddbb40ddcbf4a1d))
- Narrow layout for the details dock ([2751e05](https://github.com/BirknerAlex/kubyl/commit/2751e05807aa04327d8337b8278578b4ae48b602))
- Fixes from review ([edcab62](https://github.com/BirknerAlex/kubyl/commit/edcab628343f1584ace9db2bf589b4c77c0f21b0))

### Kubyl_kube

- Remove sources, toggle the default kubeconfig, delete pasted ones ([971acf4](https://github.com/BirknerAlex/kubyl/commit/971acf44f2441b55f7b241b6a74c224a217eeebf))
- Expose the user's bearer token per cluster ([ff2af88](https://github.com/BirknerAlex/kubyl/commit/ff2af8858512010f172e987b15bcf59e71e3aa5e))
- Keep the sign-in dialog open on Enter ([cb34ca9](https://github.com/BirknerAlex/kubyl/commit/cb34ca949c5729b7b73726dc2e994fd3ee2d993a))
- OpenShift OAuth sign-in ([c638703](https://github.com/BirknerAlex/kubyl/commit/c638703e4dda02540222ff056a1022629a24dd43))
- OpenShift OAuth review fixes ([8970514](https://github.com/BirknerAlex/kubyl/commit/897051435bf0023e60a3b9373a74419e35842239))

### Kubyl_logs

- Finish the log view, streams and Active Sessions panel ([43355c8](https://github.com/BirknerAlex/kubyl/commit/43355c8035569e8f85ceb3fedbf2353222972731))
- Detect levels after timestamps, stack traces keep their level ([17c0ba1](https://github.com/BirknerAlex/kubyl/commit/17c0ba1038fc755fa79ec16a7e959c5de7964059))
- Fixes from review ([3a6cff2](https://github.com/BirknerAlex/kubyl/commit/3a6cff26c86f3863acf20f1186fbc31897b23b76))

### Kubyl_metrics

- Prometheus discovery and client, metrics-server fallback ([9e0a604](https://github.com/BirknerAlex/kubyl/commit/9e0a604c848a9a5edffbeb77f404eba6f3ec3188))
- Network, disk and pressure metrics in the details ([a95fbea](https://github.com/BirknerAlex/kubyl/commit/a95fbea85a34cce35a31ddb038148b17339da6d6))
- Fetch less on busy clusters ([605e62d](https://github.com/BirknerAlex/kubyl/commit/605e62dbb920a6697d3374af1df2565648ce1faa))
- OpenShift monitoring through its Route ([73a73fc](https://github.com/BirknerAlex/kubyl/commit/73a73fc1974ad137022cb0729adc099c7503dd58))
- Review fixes ([8f502f6](https://github.com/BirknerAlex/kubyl/commit/8f502f6ebd7089bac09a644367f479cffe26bce7))
- Only trusted Routes get the user's token ([7bdd567](https://github.com/BirknerAlex/kubyl/commit/7bdd5679fd7c0dd922ab743dfd5338ca0efd4713))

### Kubyl_overview

- Cluster overview, events stream and Events view ([6e4e956](https://github.com/BirknerAlex/kubyl/commit/6e4e9561dac6fe201a98fc909a3b65269b685732))
- Format the live test ([0c3037e](https://github.com/BirknerAlex/kubyl/commit/0c3037e009d269a230ffbee0e5b897878606db07))
- Fit the Events toolbar and namespace tiles at dock width ([371d3d2](https://github.com/BirknerAlex/kubyl/commit/371d3d20c1859e3bb7497830b99d6699f50917fd))
- Open the Events dock with the overview, label cordoned nodes ([cf35449](https://github.com/BirknerAlex/kubyl/commit/cf3544939e3887733bb16eba750f730aa203b285))
- Shared series colors, network/throttling/disk/drop charts ([9ae4286](https://github.com/BirknerAlex/kubyl/commit/9ae4286589ec739abf6fb11a5a3b5baa4639bb95))
- The namespace overview asks only for its pods' usage ([22185cf](https://github.com/BirknerAlex/kubyl/commit/22185cf965e6e6b8d9f18fbaf6ef4e7a52c195e2))
- Review fixes ([5893693](https://github.com/BirknerAlex/kubyl/commit/58936933a4ee535a3cd0ee454c1587da4a237bf6))

### Kubyl_portforward

- Port picker, saved forwards, reconnect state, browser buttons ([522a26d](https://github.com/BirknerAlex/kubyl/commit/522a26db605c760bd40a9db42486a97b4313c876))
- Format the live test ([6362d18](https://github.com/BirknerAlex/kubyl/commit/6362d1851e8f24b04e57d0ce370fd44387858fe3))
- One-click forwards and the shared forward list ([0a6c26b](https://github.com/BirknerAlex/kubyl/commit/0a6c26bac42ee6108c5691a9819eea06ca2bacbd))
- Fixes from review ([8a67329](https://github.com/BirknerAlex/kubyl/commit/8a67329a4ed41909a9160eaac6b9fe9df9090921))

### Kubyl_resources

- List watches, pause and resume them ([0e37dec](https://github.com/BirknerAlex/kubyl/commit/0e37dec601ac9f2671cc7a0ea369626fe8fe32f0))
- Metrics::changed revision for usage observers ([6fd06db](https://github.com/BirknerAlex/kubyl/commit/6fd06dbe7ca082f3e6ca859ff49c14e425af7b17))
- MetricsProvider::source_status ([6a9233b](https://github.com/BirknerAlex/kubyl/commit/6a9233bf75a1ef28c9215ed066beb09d75be33ef))

### Kubyl_terminal

- Cell-grid renderer, attach, debug containers, node shells, dock panel ([e2c9208](https://github.com/BirknerAlex/kubyl/commit/e2c920883b7cd9296b8fc7d0db71faa0d8526949))
- Fixes from review ([d5b518a](https://github.com/BirknerAlex/kubyl/commit/d5b518afad9a24076a04b1c2a3329162e5016c43))

### Plans

- Phase 05 done ([f5c563b](https://github.com/BirknerAlex/kubyl/commit/f5c563bad55174fe0bb8f604539c029d23335a70))
- Phase 13, kubeconfig editor ([a97a9d7](https://github.com/BirknerAlex/kubyl/commit/a97a9d7d2c4cdf5333e462e114a4d673796ba195))
- Phase 06 done ([29f44b2](https://github.com/BirknerAlex/kubyl/commit/29f44b2e2aa50e60a48c26bde5966f325a6a72cf))
- Phase 13 security notes from review ([75eee88](https://github.com/BirknerAlex/kubyl/commit/75eee8881038e2e034a6a462d11f8afe588c9ee9))
- Phase 07 done; metrics, charts and events decisions and extension points ([d4561ec](https://github.com/BirknerAlex/kubyl/commit/d4561ec8df74fd937a87a202f932133902038d94))
- Phase 07 metrics details, shared chart colors, DetailsSection ([b4f425d](https://github.com/BirknerAlex/kubyl/commit/b4f425de6156083de58384a053976968c7d90055))
- Phase 07 handoff before merge ([9ca9da8](https://github.com/BirknerAlex/kubyl/commit/9ca9da819b75dfefd3d95dbe7b98e8b9bc438875))
- Phase 07 handoff for OpenShift and the review fixes ([f696700](https://github.com/BirknerAlex/kubyl/commit/f696700bd0cac9903d8e532ecb4b794efd5945eb))
- Renumber the phases after 07 ([78ab878](https://github.com/BirknerAlex/kubyl/commit/78ab878f51c4521c40999b7ac5e5524ad5becaf3))
- Phase 07 security notes, phase 13 reference ([ac993f6](https://github.com/BirknerAlex/kubyl/commit/ac993f6241673b9baa559c26ddec566764e1fab8))

### Script

- Prometheus-dev.sh (metrics-server and kube-prometheus-stack on kind) ([0364718](https://github.com/BirknerAlex/kubyl/commit/0364718af9c5ca3ae2b5a6565c54260af7fbbdc9))

## [0.1.1] - 2026-09-25

### Kubyl

- Fix macOS app icon not being bundled ([09347d2](https://github.com/BirknerAlex/kubyl/commit/09347d298fd0851242dde5497c67706d6d5d5f73))

### Kubyl_explorer

- Inline YAML/Logs/Terminal sub-tabs and Secret value reveal ([04eca7a](https://github.com/BirknerAlex/kubyl/commit/04eca7a8ca74e3cac68ff3ef894d85fea8c16d82))
- Full-width YAML/Logs/Terminal sub-tabs in Details view ([864ad46](https://github.com/BirknerAlex/kubyl/commit/864ad4675efc09f7325b584c2b9bfebf4871f7a3))
- Drop the 980px width cap on all Details sub-tabs ([33b3403](https://github.com/BirknerAlex/kubyl/commit/33b3403e23002d33db6694d586c2b55baf64435a))

### Kubyl_kube

- Fix the cluster switcher (and other title-bar actions) doing nothing ([7ac4ab0](https://github.com/BirknerAlex/kubyl/commit/7ac4ab0659353e0106e5a919b6760ad1b1c9967f))

### Kubyl_logs

- Log streaming, search, level detection, active sessions ([2d01b7c](https://github.com/BirknerAlex/kubyl/commit/2d01b7c9196fbde762a713a7f1405ea82ae54029))
- Fix CodeRabbit findings from PR #1 review ([f8bbf56](https://github.com/BirknerAlex/kubyl/commit/f8bbf561387fb31374e702f9524e28bcbee9f51f))

### Kubyl_portforward

- Pod/Service/workload forwarding, connection manager ([10e286f](https://github.com/BirknerAlex/kubyl/commit/10e286f2861cbcf176405131edfeed2de3d0d4ea))
- Fix CodeRabbit findings from PR #1 review ([c9481e1](https://github.com/BirknerAlex/kubyl/commit/c9481e1425b8fbada40ec1a246f8e8525e06f7c4))
- Fix second CodeRabbit pass findings ([809b531](https://github.com/BirknerAlex/kubyl/commit/809b5319b5b532970af3ca6ec44695eb010032da))

### Kubyl_terminal

- Alacritty_terminal grid, exec bridge, shell view ([1b17e87](https://github.com/BirknerAlex/kubyl/commit/1b17e870c0fb04ce07db4dc702024f32bb0d6ab3))
- Fix CodeRabbit findings from PR #1 review ([417f975](https://github.com/BirknerAlex/kubyl/commit/417f975e8906256d6a57854c247f4d0abfab931d))

### Kubyl_ui

- Add EyeOff icon for masked-value reveal/hide toggles ([912c47c](https://github.com/BirknerAlex/kubyl/commit/912c47ca0b782d8899ed87dac193c02662762920))

### Plans

- Update phase 05 status and handoff log ([d7c37b9](https://github.com/BirknerAlex/kubyl/commit/d7c37b968ae451ae90998d4bb3729b47cc1f9b70))
- Log the Details pane sub-tabs and Secret reveal feature ([8298b31](https://github.com/BirknerAlex/kubyl/commit/8298b31854bcc0fce1ec8d6d4b816bd53542d7ae))

### Release

- Fix version-bump push to protected main ([58dd940](https://github.com/BirknerAlex/kubyl/commit/58dd940644baded91dee3601e7d31a4106482afe))

## [0.1.0] - 2026-09-24

### Kubyl

- Native Kubernetes desktop client ([839a3dc](https://github.com/BirknerAlex/kubyl/commit/839a3dce372038191826c5d2ed4a8c881bb24063))


