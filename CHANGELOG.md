# Changelog

All notable changes to Kubyl are documented here.

## [0.2.2] - 2026-09-26

### Cargo.lock

- Kubyl_alerts at the workspace version 0.2.1 (after merging main) ([467ee10](https://github.com/BirknerAlex/kubyl/commit/467ee106a77afd617d1c86d02f2288da2d5c9907))

### Design

- Boards 16 · alerts and 17 · cluster status, grouped contexts, ConfigMap data; status dots on every sidebar ([bbba219](https://github.com/BirknerAlex/kubyl/commit/bbba219847cf338b3070f292a2a7b9f535257dc1))

### Kubyl

- Tabs follow their cluster when its id changes (grouped contexts) and are saved with current ids ([37543f0](https://github.com/BirknerAlex/kubyl/commit/37543f0555c6bdf2a33ba3beb3a8cdbe157b0bd2))
- Tabs with unsaved changes aren't rebuilt when cluster ids change (review) ([b97bee3](https://github.com/BirknerAlex/kubyl/commit/b97bee3c4f5989a77019224bef2341cdd00994c6))

### Kubyl_alerts

- Stub crate for alerts (phase 14) ([5decb38](https://github.com/BirknerAlex/kubyl/commit/5decb3822c8b15c6e7f87b48fa2a38dadc7eaa5a))
- Settings, model and parsers, matchers, merge, discovery and the Alertmanager client (phase 14) ([012b8d8](https://github.com/BirknerAlex/kubyl/commit/012b8d84b1317f1ea40541532a3aa3ce2ca299fb))
- The alerts service, the Alerts view (alerts, silences, rules), the silence editor and the chrome; alertmanager-dev.sh and live tests (phase 14) ([5622f62](https://github.com/BirknerAlex/kubyl/commit/5622f629024bc5a07a8639bea269d249f4234e76))
- Readable values, source column, collapsible rule groups; tests for transitions, notifications, optimistic writes, 5,000 alerts and Debug output (phase 14) ([3b636a5](https://github.com/BirknerAlex/kubyl/commit/3b636a551fa6ac4c7dd21099a237cd362fef1205))
- List silenced and inhibited alerts after the active ones, marked as firing with a bell-off or eye-off icon (phase 14) ([c661964](https://github.com/BirknerAlex/kubyl/commit/c661964edb02fbea7118b74b18789197f4798536))
- Silence dialog and filters at the UI text size, the preview visible, wrapped PROD warning; overview card aligned with the tiles (phase 14) ([62a9b44](https://github.com/BirknerAlex/kubyl/commit/62a9b44181c8bd8da1bdbb1d6df92dd8fd01d156))
- Keychain headers keyed by the API URL (with path, without trailing slash); no overflow in durations; service updates don't keep hidden tabs at the fast pace (review) ([4bf427e](https://github.com/BirknerAlex/kubyl/commit/4bf427e6b5cbf21868316eac028b814fca338823))

### Kubyl_argocd

- Confirmed installs keep their key when contexts are grouped (phase 15 follow-up) ([dee001c](https://github.com/BirknerAlex/kubyl/commit/dee001cea1b40fe1d7855f297de2f90be8b954c5))

### Kubyl_core

- ClusterIds resolves out-of-date cluster ids; ViewRegistry::build uses current ids (phase 15) ([569a5bc](https://github.com/BirknerAlex/kubyl/commit/569a5bcdc097d5a5b797b5e64ab1bd47cc3b8137))

### Kubyl_explorer

- Connection status slot and tooltip on cluster rows, faint favorites while disconnected, cluster order and "connected only"; expanded roots connect once kubeconfigs are loaded ([51599d7](https://github.com/BirknerAlex/kubyl/commit/51599d71537e0f27ae57eed0fb4dd9d93ca5207f))
- ConfigMap data in the details (blocks per key, formats, YAML highlighting, binaryData, key filter, copy all), "Used by" for ConfigMaps and Secrets, multi-line revealed Secret values ([3474e1b](https://github.com/BirknerAlex/kubyl/commit/3474e1b90256319ce40b39aa5af959db47ad2c41))
- Grouped clusters in the sidebar (member tooltip, show separately), favorites and the namespace picker resolve to entries ([eb0bcef](https://github.com/BirknerAlex/kubyl/commit/eb0bcefc197e8d465cd24d8229efd513dc59cda7))
- Rows other crates add under each cluster (register_view_row, with a badge, following group_order and hidden_groups) and markers on cluster root rows (phase 14) ([aede899](https://github.com/BirknerAlex/kubyl/commit/aede899bc0196ad31742a905f37ece422ac391eb))
- Keep the watched ConfigMap/Secret object on repeated selections, escape backticks in .env exports, no "Show as One Cluster" while grouping is off (review) ([e0ac831](https://github.com/BirknerAlex/kubyl/commit/e0ac83184568aae30d73363aeca9b7f7ece549c6))

### Kubyl_kube

- One entry per cluster and user (context grouping), resolve and settings_keys, re-keying without reconnecting ([790d091](https://github.com/BirknerAlex/kubyl/commit/790d091b1247ef9bd0c38ecffc84b8ede9efb8e4))
- The grouping test doesn't assume '/' as path separator (Windows CI) (phase 15) ([33ea948](https://github.com/BirknerAlex/kubyl/commit/33ea948ffbbbde0dd3880e62839d1a10a8dd4212))
- Contexts of a group shown separately keep the group's Production and Read-only flags; turning one off keeps it for the siblings (review) ([5dc04f5](https://github.com/BirknerAlex/kubyl/commit/5dc04f5fb330194b2e9d3908973c53d62a148784))

### Kubyl_metrics

- Metrics.prometheus and the keychain header through settings_keys (phase 15 follow-up) ([631dbb9](https://github.com/BirknerAlex/kubyl/commit/631dbb9ee3e4dbcaa0ca3bfcfa88ef8b9d69a633))
- The transport becomes a public module (service proxy, direct with bearer, roots and TLS server name, external URL with header and mTLS, get/post/delete); through_route takes the probe path; PromClient::api and MetricsService::prometheus (phase 14) ([884b718](https://github.com/BirknerAlex/kubyl/commit/884b718a8ef064fc87eb54aef95fb83fbc6aa493))

### Kubyl_palette

- @ contexts show the connection state like the sidebar (phase 15) ([03c9acf](https://github.com/BirknerAlex/kubyl/commit/03c9acfa4c2362a169dad1912d9ac63d83b75460))
- @ finds a group by its contexts' names and opens it in that namespace ([055f2be](https://github.com/BirknerAlex/kubyl/commit/055f2bea3ed790fb3d980d0fbedcfdf6e1e6a25f))

### Kubyl_portforward

- Saved forwards start on their grouped cluster (phase 15 follow-up) ([fa15398](https://github.com/BirknerAlex/kubyl/commit/fa153985c9780d64b7e186e54f51a9e267663a4c))

### Kubyl_ui

- Pulsing status dots, tooltips on tree rows, an end slot in section headers (phase 15) ([d124ca8](https://github.com/BirknerAlex/kubyl/commit/d124ca841c5a7898385bc395b1be110c0fb04274))
- Siren, bell-off and list-checks icons (phase 14) ([2e9a075](https://github.com/BirknerAlex/kubyl/commit/2e9a0758d5afe0b4c5b7ed7801c99fe119f56332))
- Rustfmt the toast action button (phase 14) ([110de50](https://github.com/BirknerAlex/kubyl/commit/110de501667e102c04c65d8ce4967ad2c2ab1506))

### Kubyl_webview

- Web view stores of a group keep their key through oc project (phase 15 follow-up) ([55e6864](https://github.com/BirknerAlex/kubyl/commit/55e68645955e9a9f64bb135e579e6c13c636c44c))

### Kubyl_yaml

- Apply history recorded before contexts were grouped still shows (phase 15 follow-up) ([2081bed](https://github.com/BirknerAlex/kubyl/commit/2081beda48523505056959cbbd55515bf5a85a91))
- Apply history combines the entry's and its former contexts' histories (review) ([98c13c6](https://github.com/BirknerAlex/kubyl/commit/98c13c64c2c41a9a2c84093fd3c46ae3075b237f))

### Plans

- Decisions of phases 14 and 15 (context grouping, alerts, Alertmanager credentials) ([1ad5371](https://github.com/BirknerAlex/kubyl/commit/1ad53712f308ea36e9996e9522543f511cfc38f6))
- Phase 15 parts 1-3 done (handoff so far) ([703c132](https://github.com/BirknerAlex/kubyl/commit/703c1329b6537727bb6f7ad096eab10179291774))

### Script

- Oc-contexts-dev.sh writes oc-style contexts for the kind cluster to a scratch kubeconfig ([c52d9a5](https://github.com/BirknerAlex/kubyl/commit/c52d9a557e2f808a08343cebae3a2f692a661e98))
- Oc-contexts-dev.sh refuses to overwrite its source kubeconfig or write into ~/.kube, also for files that don't exist yet (review) ([935c9e5](https://github.com/BirknerAlex/kubyl/commit/935c9e5a185798155bbd3f4db884befa2394c98e))

## [0.2.1] - 2026-09-26

### AGENTS.md

- Gotchas for dialogs in GPUI tests and on_next_frame on macOS ([be1191c](https://github.com/BirknerAlex/kubyl/commit/be1191cfb7d2b749b6788332f12bea486d874cd8))

### Design

- Board 11 · kubeconfig editor (form, YAML and test results, wizard, save preview and exec consent) ([ad00be5](https://github.com/BirknerAlex/kubyl/commit/ad00be5ba4c760c032d8179e36ae9095156e861f))
- Phase 11 screenshots (kubeconfig editor, test, wizard, save, consent) ([8f888ea](https://github.com/BirknerAlex/kubyl/commit/8f888eabdc6649afa1541dbb984258125ceac9f2))

### Kubyl

- "New kubeconfig…" in the Explorer's + menu ([cb2ec15](https://github.com/BirknerAlex/kubyl/commit/cb2ec153097d7be381091ee82ed1b9640a3edf66))

### Kubyl_core

- ViewRequest::for_path, NewKubeconfig and EditKubeconfig actions ([30aeebd](https://github.com/BirknerAlex/kubyl/commit/30aeebd81f6052c7698e017f33d7b4a246bab816))

### Kubyl_kube

- Context_info for kubeconfigs that aren't loaded, move_context_settings ([735bc4a](https://github.com/BirknerAlex/kubyl/commit/735bc4a5cd52079bafdcb957368755ddabd30f5a))
- Open the kubeconfig editor from the Clusters tab; proxy helpers public ([4f530a8](https://github.com/BirknerAlex/kubyl/commit/4f530a88aa433717c1451b94dbde07b76fd9ba4e))
- Pasted kubeconfigs list their exec plugins and need trust ([389b149](https://github.com/BirknerAlex/kubyl/commit/389b149020961ac6e7f14e77a4938d8c0f33ec06))

### Kubyl_kubeconfig

- Stub crate for the kubeconfig editor (phase 11) ([88aca01](https://github.com/BirknerAlex/kubyl/commit/88aca01e9d92b55cb464e6ac88fc1d8ec8a628b2))
- Document model, comment-preserving writer, atomic saves with backups, certificates, TLS checks and CA fetch ([98b64b2](https://github.com/BirknerAlex/kubyl/commit/98b64b221ad66bc10dc2fd00529594b7cc826430))
- Editor tab, forms, YAML tab, connection test panel, dialogs, wizard, imports ([4c68f44](https://github.com/BirknerAlex/kubyl/commit/4c68f448efc0fe5ec8ba6ab0c87495a9bfbdbf43))
- Save through the editor, folders from the global, editor and live UI tests ([953e430](https://github.com/BirknerAlex/kubyl/commit/953e430e083ae041e208f9fe5584a68985eaa6fd))
- Polish from the screenshots, service-account live test ([e082caa](https://github.com/BirknerAlex/kubyl/commit/e082caa24f2955acd96179ac847f0ac9220fd443))
- Live test for OIDC sign-in from the connection test ([df356f6](https://github.com/BirknerAlex/kubyl/commit/df356f639f594306c98757036d9e976b2abe7f5c))
- Fix an unused variable in a files test on Windows ([92846a3](https://github.com/BirknerAlex/kubyl/commit/92846a348f335960ee77b40a7948fbfefc5a83a5))
- Review fixes ([3155bef](https://github.com/BirknerAlex/kubyl/commit/3155befbc68a7d0ce455ae8fbade104e44d73e58))
- The wizard counts a test only once it ran ([6309333](https://github.com/BirknerAlex/kubyl/commit/6309333148145913201efbe6419d40abcf8c1e49))

### Plans

- Phase 11 decisions: which kubeconfigs Kubyl writes, comments, credentials, consent ([050fcac](https://github.com/BirknerAlex/kubyl/commit/050fcac4372e5345c565445cdbcf6de5fa501679))
- Phase 14, alerts (Alertmanager) ([37d61b2](https://github.com/BirknerAlex/kubyl/commit/37d61b2b1ba3c3b584e1c45b64eca0e48cd3c64d))
- Phase 15, polish (connection dots, context grouping, ConfigMap data) ([757852f](https://github.com/BirknerAlex/kubyl/commit/757852f6abe26552e034238ae3246c94a8d72bfe))
- Phase 11 done (handoff log, decisions, extension points) ([c0951b3](https://github.com/BirknerAlex/kubyl/commit/c0951b3cff6f612a24d4ee3a7ca80797ea4ceb42))
- Phase 15 label examples with made-up hosts and user ([66341f6](https://github.com/BirknerAlex/kubyl/commit/66341f68166ea1561e5e204dd98a550310fd0af9))
- Phase 11 handoff and decision table: second hash check, masks ([a5e5c3a](https://github.com/BirknerAlex/kubyl/commit/a5e5c3a3028a2a7836068cf1db0b786fcd41f2f1))

### Workspace

- Rustls, rustls-pki-types and tokio-rustls as direct dependencies (versions and features kube already uses) ([239c382](https://github.com/BirknerAlex/kubyl/commit/239c3821e2ca1107b56d9f18ea7c68d942ca52b7))

## [0.2.0] - 2026-09-25

### Design

- Argo CD boards (applications, resource tree, history and rollback, sync) ([0d8c65d](https://github.com/BirknerAlex/kubyl/commit/0d8c65da7359904c8ddc1b3681f383c23f2753ef))

### Kubyl

- Show tab dots, close tabs that ask for it ([79c460c](https://github.com/BirknerAlex/kubyl/commit/79c460ce4719a8ac9d8288ff3f6951208273df69))
- Web view snapshots in harness screenshots ([8bd0132](https://github.com/BirknerAlex/kubyl/commit/8bd0132aa3a3df9460aa01bb9ca157402ffb3a11))
- Screenshot runs opt out of App Nap (macOS) ([2d46cc7](https://github.com/BirknerAlex/kubyl/commit/2d46cc76842ebbe02630caf7773ac89f6f5c8a3c))
- Screenshot waits don't depend on dispatch timers ([59cf4c1](https://github.com/BirknerAlex/kubyl/commit/59cf4c1a889708eb0816bacbd237223737712c8f))

### Kubyl_argocd

- Stub crate for phase 10 (Argo CD) ([4807e50](https://github.com/BirknerAlex/kubyl/commit/4807e501f59611cf334f563e5f5fb9c676de1ee3))
- Model, Kubernetes-mode actions, API client, detection ([ff79abd](https://github.com/BirknerAlex/kubyl/commit/ff79abdb7e4045a8215eddc7becdb8e9e3ca58e1))
- Applications, application tab, dialogs, dock, actions ([8c4505a](https://github.com/BirknerAlex/kubyl/commit/8c4505ae08fd133937a671a5c9132faddc539d45))
- Fixes from running against kind ([bfdcff5](https://github.com/BirknerAlex/kubyl/commit/bfdcff5f37616e633c75759f673d71e21c682af8))
- UI follows the CRDs; live UI test ([563b824](https://github.com/BirknerAlex/kubyl/commit/563b82420c11cced0e48a921caae831fd7496488))
- Rollback options align; mode tooltip says what API mode adds ([eeaaed0](https://github.com/BirknerAlex/kubyl/commit/eeaaed0db7b759113dc55e3d3f1f6e88f92bea42))
- SSO sign-in, and the Argo CD UI opens signed in ([3d5cb79](https://github.com/BirknerAlex/kubyl/commit/3d5cb79ad014a6fa965d73f22ae630f99c6e0651))
- SSO sign-in survives the dialog; explains confidential clients ([1ff7df6](https://github.com/BirknerAlex/kubyl/commit/1ff7df61dc43b727389fa42c5ca1d96e29c031bb))
- Fixes from review ([e2357b3](https://github.com/BirknerAlex/kubyl/commit/e2357b3a3cd3b59a93cea6c19d663ef207504519))
- SSO in the browser only; Argo CD UI at its own URL for SSO ([d3461c5](https://github.com/BirknerAlex/kubyl/commit/d3461c593535e2bb98c0dfce5ddca7e62f43a401))
- Signing out revokes the Argo CD session ([48ce535](https://github.com/BirknerAlex/kubyl/commit/48ce535f276a2e15742122b562d4521d1ab695fe))

### Kubyl_core

- Button cells and extra columns for existing kinds ([f8f8c19](https://github.com/BirknerAlex/kubyl/commit/f8f8c194899888b838d9c7db955875140750af02))
- Argo CD caps, list/object view overrides, edit notices ([f3bb46c](https://github.com/BirknerAlex/kubyl/commit/f3bb46c6271684cf09b33fc518f7c382fd29d708))

### Kubyl_explorer

- Contributed tree groups, kind-specific object views ([e0226dd](https://github.com/BirknerAlex/kubyl/commit/e0226dd17e47dba6829997162eb560ab5855e487))
- PVC details list the pods using the claim ([001daf5](https://github.com/BirknerAlex/kubyl/commit/001daf59540465a1ccaf0a0a750510db2f6b9d71))
- Rustfmt ([7daf460](https://github.com/BirknerAlex/kubyl/commit/7daf4604e27a0428901646b54289db0a7cf70ab9))

### Kubyl_kube

- Fill ClusterCaps::argocd from discovery ([09f1647](https://github.com/BirknerAlex/kubyl/commit/09f1647409e683633dd1f7ba6152e9c668a2e214))
- Re-run discovery until new CRDs are served ([33ea83b](https://github.com/BirknerAlex/kubyl/commit/33ea83bc91a52a3572df5796a0e651a5ced15f8a))
- Loopback redirect helpers and jwt_expiry are public ([23f9d79](https://github.com/BirknerAlex/kubyl/commit/23f9d79b0dc95ab271f908a3f19cfde3047e6db7))

### Kubyl_logs

- Open_filtered opens a log view with a search applied ([809643b](https://github.com/BirknerAlex/kubyl/commit/809643b147e0c23eae95885b2e47056e974b2d62))
- Open_filtered can search with a regular expression ([4e632b5](https://github.com/BirknerAlex/kubyl/commit/4e632b5399f5aa8ddf7e59aef1667e19386fa366))

### Kubyl_palette

- Open kinds and objects in their registered views ([3243d20](https://github.com/BirknerAlex/kubyl/commit/3243d20dc51907d33d3fa24d13c39bd4cc47faa0))

### Kubyl_portforward

- Temporary forwards for other crates ([3f52024](https://github.com/BirknerAlex/kubyl/commit/3f52024e78ae5dbe01cda7bdc35e61934560c224))

### Kubyl_ui

- Icons for Argo CD (branch, commit, fork, history, rollback…) ([781c32c](https://github.com/BirknerAlex/kubyl/commit/781c32c4c222726a8b6400da12929372726c7572))
- Check icon ([6b1517d](https://github.com/BirknerAlex/kubyl/commit/6b1517debb8e3bd45587bc235c16b0df5057970f))

### Kubyl_webview

- Stub crate for phase 08 ([b8f285f](https://github.com/BirknerAlex/kubyl/commit/b8f285f7c104e1cac471c002162bf62e6b49aadd))
- Service web views over temporary port-forwards ([a3c6d80](https://github.com/BirknerAlex/kubyl/commit/a3c6d80ac73fc709f57e89497cebce4e697d0ec6))
- Linux web views, macOS fixes ([d4d1a6d](https://github.com/BirknerAlex/kubyl/commit/d4d1a6daa0e9e3c19d66d13377a6869d15531315))
- Idle and download fixes, timings ([0d3c943](https://github.com/BirknerAlex/kubyl/commit/0d3c94353dfb68752b3c3d1a627ac450f36006f6))
- Typing a path in the address bar, closed tabs let go ([e23adcb](https://github.com/BirknerAlex/kubyl/commit/e23adcb8481f5fa6b5d99cf43110db7d24698c83))
- Review fixes ([3be0951](https://github.com/BirknerAlex/kubyl/commit/3be0951ee7b7383f85c114ce2951899c8201c104))
- Don't abort on a page without a URL (macOS) ([49ab311](https://github.com/BirknerAlex/kubyl/commit/49ab31181b75133732db9a86f219b9d2fb7aeec1))
- Session cookies from crates holding a session for a target ([cfb2b8a](https://github.com/BirknerAlex/kubyl/commit/cfb2b8a892a6c00a23adc16c948f53e390dd8f08))
- Session cookies on macOS without blocking ([b3cb5f5](https://github.com/BirknerAlex/kubyl/commit/b3cb5f5f2eb36734a22a194f8412c78760319d02))
- MacOS session cookies that older WebKit takes ([5d0bb15](https://github.com/BirknerAlex/kubyl/commit/5d0bb157aeb803f61bb9d1d9298d5f7911f19030))
- Session cookies try wry's set_cookie first on macOS ([79afd27](https://github.com/BirknerAlex/kubyl/commit/79afd270f58b97f1d8521b171751ea89ac2bf242))
- Live_webview says what the store holds when a session cookie is missing ([7f4fbdd](https://github.com/BirknerAlex/kubyl/commit/7f4fbdddcc5172bb488c1bfb0bbfa426cf536de2))
- MacOS session cookies are secure only for https ([b214310](https://github.com/BirknerAlex/kubyl/commit/b21431085f7afa9f95deccca7f65fe71e86886fa))

### Kubyl_yaml

- Reusable diff view, edit notices as a banner ([c6f6b93](https://github.com/BirknerAlex/kubyl/commit/c6f6b936ca7123ef030f2b44051c10537a0ff295))

### Plans

- Phase 08 done, web view decision and screenshots ([8c0fe67](https://github.com/BirknerAlex/kubyl/commit/8c0fe670fd6f5f2d3848e0273eabf3a1e9e47777))
- Phase 10 done (Argo CD), screenshots, README decisions and extension points ([8cad2ac](https://github.com/BirknerAlex/kubyl/commit/8cad2ac21a2efc86950a7a331c6a875ef9f32e09))
- Phase 10 SSO, signed-in Argo CD UI, PVC pods; screenshots ([cab6c74](https://github.com/BirknerAlex/kubyl/commit/cab6c74d1030a31908270f4e7470b679b5c15499))
- Phase 10 sign-out revokes the session ([f93dfe8](https://github.com/BirknerAlex/kubyl/commit/f93dfe84b6a97ed3c7b0cd96cb0f3a9ea502d57b))

### Script

- Webview-dev.sh for trying service web views ([7b8dbfa](https://github.com/BirknerAlex/kubyl/commit/7b8dbfa89b766830a2f865d585285d1d3fb76dfa))
- Argocd-dev.sh installs Argo CD with sample apps on kind ([f38132f](https://github.com/BirknerAlex/kubyl/commit/f38132f827795a7b8cc16b52c952706f9a4d3098))

### Workspace

- Web view dependencies ([84a2595](https://github.com/BirknerAlex/kubyl/commit/84a2595cd9563c4ce57938949326b4605bd567d7))
- Gtk and webkit2gtk for Linux web views ([e88cef7](https://github.com/BirknerAlex/kubyl/commit/e88cef75f09c99c342129b6f5ab83708925d3a36))
- Wry's devtools feature ([35f33e5](https://github.com/BirknerAlex/kubyl/commit/35f33e57509f622cef1fa8db5543779ecd22af47))

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


