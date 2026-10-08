# Changelog

All notable changes to Kubyl are documented here.

## [0.6.0] - 2026-10-08

### Bug Fixes

- Address CodeRabbit review on phase 21 (review) ([5f60d44](https://github.com/BirknerAlex/kubyl/commit/5f60d44dcedf3f429810dd8ded39a83b9485a49e))

### Features

- Agents over ACP with Kubyl's cluster tools (phase 21) ([96ddb5f](https://github.com/BirknerAlex/kubyl/commit/96ddb5fcbe09aa9adea0b411db34e75fbb48d47f))
- One Credentials handle for every stored secret, with optional scopes ([a14a655](https://github.com/BirknerAlex/kubyl/commit/a14a655ead68d155498e15cd547c66cfc5fbf706))
- Scoped credential handles ignore tokens written into the kubeconfig ([6740fdb](https://github.com/BirknerAlex/kubyl/commit/6740fdb5e5600c3c081fe9c9a38222f5094137d7))
- Flux CD sources, Kustomizations, HelmReleases, reconcile and suspend (phase 23) ([ac26db5](https://github.com/BirknerAlex/kubyl/commit/ac26db5da599818e8e73aa9779b72d31d8490306))
- Charts, install, upgrade, rollback and uninstall (phase 22) ([9546aef](https://github.com/BirknerAlex/kubyl/commit/9546aefc1fe531803d53c8607cd31d1293310d6c))
- Device resources, admission policies, Gateway API, VPA and EndpointSlice views (phase 24) (#45) ([2fa5584](https://github.com/BirknerAlex/kubyl/commit/2fa558430d15da894871332839f3edefae581a02))

## [0.5.0] - 2026-10-06

### Bug Fixes

- Base64-encode the client certificate of an exec plugin ([5770b18](https://github.com/BirknerAlex/kubyl/commit/5770b18eb17a3e25be7b9fe62fd566f1586c9b96))
- Let service leases cross threads and stores be copied from other threads ([eab38a4](https://github.com/BirknerAlex/kubyl/commit/eab38a47e0d3d9be3442883aec4c9b3576f63855))
- Address CodeRabbit review on phase 20 ([0ee88c9](https://github.com/BirknerAlex/kubyl/commit/0ee88c9156abe83ecb7b41c803f46776464933ba))

### CI

- Sync winget-pkgs fork before submitting manifest (#34) ([5cf2f4a](https://github.com/BirknerAlex/kubyl/commit/5cf2f4a4aca9d36b308e7c5b55622d7b4b3f372f))

### Documentation

- Require the contributor license agreement on kubyl.dev (#35) ([989df60](https://github.com/BirknerAlex/kubyl/commit/989df60d525d8657681cc12c6434ca6be6fefa1a))
- Mark phase 18 (Prometheus) as done ([7f286d0](https://github.com/BirknerAlex/kubyl/commit/7f286d0449e4f632cbe39782ca9add0e9efed6ed))
- Add phase 19, split domain logic from the UI ([a6142b1](https://github.com/BirknerAlex/kubyl/commit/a6142b10cb83e913d1e1605c502b122da7eebfb0))
- Phase 19 progress and the core-crates decision ([2a784fe](https://github.com/BirknerAlex/kubyl/commit/2a784fed2cfac008399bfef208fb86a0844d50f2))
- Phase 19 status, workspace layout and where logic goes ([e894102](https://github.com/BirknerAlex/kubyl/commit/e894102135b605027e0dc925a3058ce8e9265cc8))
- Phase 19 checked against the dev cluster ([cb18ffc](https://github.com/BirknerAlex/kubyl/commit/cb18ffca83c39570a8e822d8972d9a8f621a698c))
- The core-crates check in AGENTS.md and CLAUDE.md ([fae3e7a](https://github.com/BirknerAlex/kubyl/commit/fae3e7a1965bb1c152a475b0452c4ab05f989dcf))
- Git rules for a public repo with PRs and the CLA ([1d4873a](https://github.com/BirknerAlex/kubyl/commit/1d4873af522919ac5521088152b559495dbb3472))
- Phase 20, the rest of the services on Host ([e2567ff](https://github.com/BirknerAlex/kubyl/commit/e2567ffed4558ebe7fc0bd8c17ed8772a54dd669))

### Features

- Let an app install its own credential store ([a2a720b](https://github.com/BirknerAlex/kubyl/commit/a2a720b1b5f1207e5a3ad0a5a083ee8e088bbddb))
- Let TestHost run until a condition holds ([c93bf9d](https://github.com/BirknerAlex/kubyl/commit/c93bf9d2ebad6cc06c9687796240497f65f16718))
- Let features reach a Service through a trait ([1d3b0aa](https://github.com/BirknerAlex/kubyl/commit/1d3b0aa69b75d097f62567ca8050f6837f32bd38))
- Store sources, change sets and the built-in columns in the core ([b7b8d00](https://github.com/BirknerAlex/kubyl/commit/b7b8d008bfef03fe2df2c98abb61df90c8006b32))
- Report a session's exit code ([43ce089](https://github.com/BirknerAlex/kubyl/commit/43ce08987a7695a1f903010e3d4df86d097f16a5))

### Miscellaneous

- Cargo.lock for the new crate dependencies ([d755f06](https://github.com/BirknerAlex/kubyl/commit/d755f06cbde15b9ece52ad56372541ed725878ea))

### Refactor

- Add kubyl_base, the GPUI-free foundation ([8c8c35e](https://github.com/BirknerAlex/kubyl/commit/8c8c35e692209af0300eb329535837ab8054d8d0))
- Add kubyl_settings_core ([1b5b576](https://github.com/BirknerAlex/kubyl/commit/1b5b576d78c794e2d5025502e22e85b4317ae492))
- Move kubeconfig, auth, clients and discovery to kubyl_kube_core ([f7a8434](https://github.com/BirknerAlex/kubyl/commit/f7a843441fea882367f007d7fb69b7a737613de3))
- Move the ConnectionManager state machine to kubyl_kube_core ([17d495b](https://github.com/BirknerAlex/kubyl/commit/17d495bc369d1f5c66110e98941ea11811d64a55))
- Add kubyl_resources_core ([ab80227](https://github.com/BirknerAlex/kubyl/commit/ab802273f96dddb83aaefe5196131b6352697fac))
- SharedString, ActiveForward in kubyl_base ([770ed64](https://github.com/BirknerAlex/kubyl/commit/770ed6468b9a9d6e67b0d4a7c30c13583ad4e95c))
- Add kubyl_logs_core ([ccfdcfe](https://github.com/BirknerAlex/kubyl/commit/ccfdcfe505000463f155289d7d9593097667593a))
- Add kubyl_terminal_core ([2307e8c](https://github.com/BirknerAlex/kubyl/commit/2307e8c722a2576ab4570be5940e0af7e876638c))
- Add kubyl_portforward_core ([1cbf3d3](https://github.com/BirknerAlex/kubyl/commit/1cbf3d3216d3b340b573f01aa5b7807865b20995))
- Add kubyl_yaml_core ([4cf8d35](https://github.com/BirknerAlex/kubyl/commit/4cf8d35bbc99b034bd9215493e9e17b1c85cc7a3))
- Move discovery, queries and transports to kubyl_metrics_core ([14a2810](https://github.com/BirknerAlex/kubyl/commit/14a2810d0080cc09ca74dc8b19cc58385ccbaf88))
- Add kubyl_charts_core ([69e9573](https://github.com/BirknerAlex/kubyl/commit/69e957317ce9577ef6665a16f2191853c83fe5f4))
- Move the metrics cache to kubyl_metrics_core ([ce2727c](https://github.com/BirknerAlex/kubyl/commit/ce2727c2267245f2bd140be7001abeb674f86fe9))
- Move clients, model, matchers and rows to kubyl_alerts_core ([b0e7559](https://github.com/BirknerAlex/kubyl/commit/b0e7559c4e1529ecdfc6dc8ba4ae2db8a87a06a7))
- Move the API, model, health, trees and ops to kubyl_argocd_core ([288230b](https://github.com/BirknerAlex/kubyl/commit/288230bd89fc623327c05268d4a9b7063434643f))
- Add kubyl_operators_core ([9c15fb2](https://github.com/BirknerAlex/kubyl/commit/9c15fb2ce7e77567ee15bd61ce609751d6d2c4d8))
- Move providers, checks and pre-flight to kubyl_updates_core ([8c75824](https://github.com/BirknerAlex/kubyl/commit/8c758249b88e2937d2e61d51c6b616089c01cb09))
- Move fetches, model and completion to kubyl_prometheus_core ([7b08233](https://github.com/BirknerAlex/kubyl/commit/7b08233a870a8943e5315dc15d26e5ae8cb92cf6))
- Move backends, model, filters and topology to kubyl_netflow_core ([32b6504](https://github.com/BirknerAlex/kubyl/commit/32b65044a5d33f966e8fb35a7ec52ac800304dc7))
- Add kubyl_files_core ([fd3b3e1](https://github.com/BirknerAlex/kubyl/commit/fd3b3e11331ed2ccda5ae52810e1bd2a4884da0c))
- Add kubyl_kubeconfig_core ([51a2e69](https://github.com/BirknerAlex/kubyl/commit/51a2e691ab963793f2ee3565e2b2abaf587bc6b6))
- Add kubyl_selfupdate_core ([5a2f5a2](https://github.com/BirknerAlex/kubyl/commit/5a2f5a2a21676f4e584835195ce6c170b8ce43f2))
- Add kubyl_palette_core ([4c49fb3](https://github.com/BirknerAlex/kubyl/commit/4c49fb3998759fb6aa106cff3c48829055e01d33))
- Add kubyl_overview_core with the event rows and settings ([bbeb9e4](https://github.com/BirknerAlex/kubyl/commit/bbeb9e4d382d5e849077098e8fa2d3087175ff91))
- Add kubyl_webview_core ([089eb72](https://github.com/BirknerAlex/kubyl/commit/089eb72752e7530742cf528b283ad6787db46500))
- Add kubyl_explorer_core with RBAC checks and settings ([71da1bf](https://github.com/BirknerAlex/kubyl/commit/71da1bf1bf1ee57be11dabe11d0a09a2f0f132ba))
- The check/download state machine runs on Host ([663ff1e](https://github.com/BirknerAlex/kubyl/commit/663ff1e13701f19fcff01f390f74c0b6b98c9443))
- Move server kinds, access and discovery to the core ([8ee7a3d](https://github.com/BirknerAlex/kubyl/commit/8ee7a3d0264aa032717c26b5bab6b0bcac67969a))
- Move update states and building providers to the core ([f19b142](https://github.com/BirknerAlex/kubyl/commit/f19b1424430d8055aa06b39577823b7ba77dc708))
- Move counts, sources and the fetch-and-merge to the core ([626b14f](https://github.com/BirknerAlex/kubyl/commit/626b14ff1046f10f69a52c8520d73eafcf7e2c56))
- Move flow states and Relay TLS rules to the core ([884fa69](https://github.com/BirknerAlex/kubyl/commit/884fa69dc7ec7a384f1d37dfacdf72ab0006720c))
- Move settings, app rows and operations to the core ([022af54](https://github.com/BirknerAlex/kubyl/commit/022af54a7dc8be955dd2a5130d09919e3d23afbd))
- Move OLM availability, snapshots and watch problems to the core ([339f20b](https://github.com/BirknerAlex/kubyl/commit/339f20bcd0b773404c0831765a9d5f7065122156))
- The updates service is UpdatesCore on a Host ([2ebec6f](https://github.com/BirknerAlex/kubyl/commit/2ebec6fea4bc23d549c73addba248b8e5a2dca18))
- The flow service is FlowsCore on a Host ([a994367](https://github.com/BirknerAlex/kubyl/commit/a994367022414910a92dceee7112d8ebb6b08ad3))
- The Argo CD service is ArgoCore on a Host ([2f42fd4](https://github.com/BirknerAlex/kubyl/commit/2f42fd4c7637f43be02ebdd78bdd8718cb6386dd))
- The alerts service is AlertsCore on a Host ([296acda](https://github.com/BirknerAlex/kubyl/commit/296acda5f402717166daf529cfdb26b5518857d7))
- The Prometheus service is PrometheusCore on a Host ([ed0fe49](https://github.com/BirknerAlex/kubyl/commit/ed0fe493b84c29172aaf2d3a4c8ac9a34fa5c2bc))
- The OLM and Helm services are OlmCore and HelmCore on a Host ([c00304e](https://github.com/BirknerAlex/kubyl/commit/c00304e434b26d238e301d63cb3e509e800a7f29))

## [0.4.0] - 2026-10-03

### Bug Fixes

- Copy buttons for dialog values that can't be selected ([9851100](https://github.com/BirknerAlex/kubyl/commit/985110041678bc02f430fa327fc5a8588f0fd602))
- Selection bookkeeping resets itself, holds runs weakly ([411dc4d](https://github.com/BirknerAlex/kubyl/commit/411dc4dd37c2b790e6e2948a39a10f9261462402))
- A value that wraps stays on its key's line when copied ([aacad27](https://github.com/BirknerAlex/kubyl/commit/aacad27f8262ef61ed1ad69348005fe11148f4bd))
- Only a row-high content mask is replaced for drag scrolling ([7226cfd](https://github.com/BirknerAlex/kubyl/commit/7226cfd37f29b6989c196d94b9450eaca3a93988))
- Cmd+A in the resource list selects the last pane pressed in ([10c7081](https://github.com/BirknerAlex/kubyl/commit/10c7081915212cdea09d5afa21fa6ce9f0e1a469))
- No ticking duration for a finished or suspended Job without an end time ([5b69c9f](https://github.com/BirknerAlex/kubyl/commit/5b69c9f611548a7dc15bd22f1337f5243abd9dd7))
- Job history orders by creation and shows store problems ([9eadfe4](https://github.com/BirknerAlex/kubyl/commit/9eadfe4765ed6b26c755287cd590f4587ecb053a))
- Dedupe equal ids per frame generation, prune closed scopes ([5df5895](https://github.com/BirknerAlex/kubyl/commit/5df5895f6a81445d590adc5baa17a78602709bc7))

### Documentation

- Selection model and screenshot steps ([dbea448](https://github.com/BirknerAlex/kubyl/commit/dbea448ed3e67e4194599467e85af476fa5f88a1))

### Features

- Selectable text with per-frame document order, scopes and select-all ([49c7774](https://github.com/BirknerAlex/kubyl/commit/49c77748a6c80229debaf06cb417c4fdd005edc6))
- Selection frame and select-all wiring ([2ae9b5b](https://github.com/BirknerAlex/kubyl/commit/2ae9b5b9455027011dec32379959297693d57e68))
- Selectable details, Describe tab and table Cmd+C ([d25988b](https://github.com/BirknerAlex/kubyl/commit/d25988b70535a644d1d4ce37c3b3fa8075d9f556))
- Select text in the alerts, Argo CD, netflow, OperatorHub, updates and web view panes ([cfbd126](https://github.com/BirknerAlex/kubyl/commit/cfbd126b51a84ec3f24b626e53fc8836e587b5a6))
- Shared job helpers and kubectl status order ([66d117e](https://github.com/BirknerAlex/kubyl/commit/66d117eef8377c8fea8806c369bc7a33267c715a))
- Job history in CronJob details ([6dcdb3a](https://github.com/BirknerAlex/kubyl/commit/6dcdb3a29d026e618adff2f1bd4ab3c5aaded83e))
- Double and triple click screenshot steps ([1dc19ae](https://github.com/BirknerAlex/kubyl/commit/1dc19ae4ece673dae44fc552913c007c0f4691fc))

### Miscellaneous

- Update Flatpak manifest to v0.3.10 ([de16e34](https://github.com/BirknerAlex/kubyl/commit/de16e340d83e2f765fa454c1fae565f784727cba))
- Drop Flathub/Flatpak packaging and point README at the docs site ([b6c906d](https://github.com/BirknerAlex/kubyl/commit/b6c906d34cfe9e636784283464d331f72300f754))

### Testing

- Font-independent selection tests ([9c56430](https://github.com/BirknerAlex/kubyl/commit/9c56430e0c6cc565b1578e89ea4a6872c8f8a2d9))

## [0.3.10] - 2026-10-03

### Miscellaneous

- Update Flatpak manifest to v0.3.9 ([2abdace](https://github.com/BirknerAlex/kubyl/commit/2abdaceedf043105aaf6ac1d1df9e588d44c89a8))

### Plans

- Note the XWayland probe and DMA-BUF workaround ([76b19ca](https://github.com/BirknerAlex/kubyl/commit/76b19ca69fc78fe770f5a50caba0b8a1cc8968a5))

## [0.3.9] - 2026-10-02

### Miscellaneous

- Update Flatpak manifest to v0.3.8 ([0dfc6da](https://github.com/BirknerAlex/kubyl/commit/0dfc6da61a86cebfbd85ed5f35836bcc485a5ef7))

### Build

- Link the MSVC runtime statically on Windows ([3127470](https://github.com/BirknerAlex/kubyl/commit/312747019fd246a31e570714bf7d7be072bd2cd5))

### Kubyl_alerts

- Sign in to Alertmanagers behind basic auth ([fda94a2](https://github.com/BirknerAlex/kubyl/commit/fda94a2fc672d8c7cde492e8610234fc91b41b18))

### Kubyl_argocd

- Keep the password of a password sign-in to renew the session ([dd5b720](https://github.com/BirknerAlex/kubyl/commit/dd5b720ef3f3a11ac6e273026ef73edeb5237f61))

### Kubyl_metrics

- Tell a basic-auth 401 apart from other 401s ([aa868c0](https://github.com/BirknerAlex/kubyl/commit/aa868c073e63a1da8b3cdf552e010365e1b71f31))

### Kubyl_prometheus

- Sign in to Prometheus servers behind basic auth ([e1271b3](https://github.com/BirknerAlex/kubyl/commit/e1271b3d7fd0d10ab5d18397d6cd21bff5a982a0))

### Plans

- Decide how basic-auth credentials are handled ([5a31383](https://github.com/BirknerAlex/kubyl/commit/5a31383bd9bf41311ad359365b6c0297a5847a98))

## [0.3.8] - 2026-09-30

### Bug Fixes

- Show Argo CD on clusters whose argoproj.io prefers another version ([21bce48](https://github.com/BirknerAlex/kubyl/commit/21bce48a30705e5be953aacf6556075314ac0268))

### Miscellaneous

- Update Flatpak manifest to v0.3.7 ([a3b55a7](https://github.com/BirknerAlex/kubyl/commit/a3b55a7b284fa87176d176e2f48fdb03eb439506))

### Kubyl

- Wire in kubyl_prometheus ([0f76f22](https://github.com/BirknerAlex/kubyl/commit/0f76f22a76c309694de9b87df5c056f177dc9557))

### Kubyl_explorer

- Favorite any view; reorder sidebar folders and clusters ([e0bd276](https://github.com/BirknerAlex/kubyl/commit/e0bd27629004cd4c78f81e117d59c4d96276149c))

### Kubyl_prometheus

- A Prometheus web UI for Prometheus, Thanos Query and VictoriaMetrics ([d0f97fd](https://github.com/BirknerAlex/kubyl/commit/d0f97fd982af245187e029682edcc3b867e3ea21))

### Kubyl_ui

- Add the Flame icon ([3d1ff25](https://github.com/BirknerAlex/kubyl/commit/3d1ff25ac87a550b736f055d571c5601a3463739))
- TabBar::tab_element for tabs wrapped in another element ([0b95af5](https://github.com/BirknerAlex/kubyl/commit/0b95af5aa0b0475e2b53e64237c4cd4c065e9f68))

## [0.3.7] - 2026-09-30

### Miscellaneous

- Update Flatpak manifest to v0.3.6 ([7b597c7](https://github.com/BirknerAlex/kubyl/commit/7b597c7be94c4f93baea910d79d3543bf4822a25))

### Kubyl_kube

- Only watches and log follows share the HTTP/2 connection ([9a8e383](https://github.com/BirknerAlex/kubyl/commit/9a8e38377d0b3fafc522f253f0c71b39d56840fa))
- Re-run discovery only when a CRD changes what discovery sees ([75b70f4](https://github.com/BirknerAlex/kubyl/commit/75b70f40d015ee128616cda765dba1e8a7e60105))
- Forget cached CRD shapes that changed during a relist (review) ([ba82fba](https://github.com/BirknerAlex/kubyl/commit/ba82fba63ce72889c096e15f27b5fd1fe23f5550))

## [0.3.6] - 2026-09-29

### Miscellaneous

- Update Flatpak manifest to v0.3.5 ([abe0c71](https://github.com/BirknerAlex/kubyl/commit/abe0c7193b07638e01213a1544eaef352f995dcd))

### Terminal

- Bound the output queue so Ctrl+C stops a flood promptly ([1fcb54b](https://github.com/BirknerAlex/kubyl/commit/1fcb54bfb3594fc7f4d9a4ca3caf72cf0d73dd7f))

### Kubyl

- Raise the open-file limit at startup ([40992c7](https://github.com/BirknerAlex/kubyl/commit/40992c7422677ba890f4a1d8a58e497b104cd92d))
- Log when the open-file limit can't be raised (review) ([f5aeda9](https://github.com/BirknerAlex/kubyl/commit/f5aeda9093a54f73b745fd2682349267b980cf09))

### Kubyl_kube

- HTTP/2 to API servers, one connection per cluster ([8a1f17e](https://github.com/BirknerAlex/kubyl/commit/8a1f17e6c561c0e8361571427894a2ff36e2c06a))

## [0.3.5] - 2026-09-29

### DataTable

- Rows fill the width so cells line up with the header ([bd78836](https://github.com/BirknerAlex/kubyl/commit/bd7883628bc6929a57fc5aca7f9bc821b119ae22))

### Logs

- Unwrapped lines scroll sideways, with a horizontal scrollbar ([7600610](https://github.com/BirknerAlex/kubyl/commit/76006100b9d655414e799d4b1aa8cecbb317f5c0))

### Miscellaneous

- Update Flatpak manifest to v0.3.4 ([a40a293](https://github.com/BirknerAlex/kubyl/commit/a40a293e7becde3491311d7df5a7f00092f821d8))

### Topology

- Nodes keep their place and the view its fit across refreshes ([c583fd8](https://github.com/BirknerAlex/kubyl/commit/c583fd845f929bb33dda76d491a36e7fc7467622))

## [0.3.4] - 2026-09-28

### Bug Fixes

- No panic completing after a multi-byte space in the flow filter ([9a4e9dd](https://github.com/BirknerAlex/kubyl/commit/9a4e9ddc09008b5b21b1c36c2d92ee9bcbbc3b4f))
- Check read-only, RBAC and PROD for every cluster in a selection ([0b6177e](https://github.com/BirknerAlex/kubyl/commit/0b6177e77c04d0efee02056358d025c792e0fe25))
- Never let a container file name escape the download folder ([31948f9](https://github.com/BirknerAlex/kubyl/commit/31948f9698228d299c506180c36b247d9de2750f))
- Reject unsafe entry names in drag-out staging and image preview ([269b868](https://github.com/BirknerAlex/kubyl/commit/269b86863117b9a3b51f0667c94277742a25901b))
- Keep draining tar stdout when local extraction fails ([ece1069](https://github.com/BirknerAlex/kubyl/commit/ece10696f4ee1ccc82ae1e8bc133351f11bf50d5))
- Don't truncate container files on a failed write ([7ffd036](https://github.com/BirknerAlex/kubyl/commit/7ffd036fec7a195b5ccf62d4327c7e374047cbd7))
- Only resume a partial download of the same remote file ([d567a68](https://github.com/BirknerAlex/kubyl/commit/d567a68bd6a7b7e9b0592650833f1c6c0dbe5c39))
- Don't panic when the preview temp folder can't be created ([1e14d37](https://github.com/BirknerAlex/kubyl/commit/1e14d37772bd1ae4075e512c63288f6f73f57aba))
- Move blocking local filesystem calls off the UI thread ([8ebd388](https://github.com/BirknerAlex/kubyl/commit/8ebd3884c470ddd36ee9de064bf25a2c35972e57))
- Strip every ESC from bracketed pastes ([2e7ae57](https://github.com/BirknerAlex/kubyl/commit/2e7ae577e3b31cb17ca4d1f9572d301ef089ab79))
- Cap terminal output batches at 256 KiB ([f50456d](https://github.com/BirknerAlex/kubyl/commit/f50456d015522001cb66c6665f1ee53ef68e0d7a))
- Clean up privileged node-shell pods on quit, failure and aborted create ([4477c91](https://github.com/BirknerAlex/kubyl/commit/4477c91ad37aabbce1f0bf9cfa42de571eb15d86))
- Refuse cmd.exe metacharacters in cloud CLI arguments ([ff7ec16](https://github.com/BirknerAlex/kubyl/commit/ff7ec16301db4aa2e75e0cc8822918ecb62f8bd3))
- Validate the EKS region before building the endpoint host ([af33a6b](https://github.com/BirknerAlex/kubyl/commit/af33a6b9c7d4e30907114ed93fb128d74b055145))
- SUC updates only to versions from the fetched channel list ([00a50c6](https://github.com/BirknerAlex/kubyl/commit/00a50c68d7a4ed31cf3cef9055f88024340cbadf))
- Confirm dialog waits for the pre-flight run and follows its results ([41137a7](https://github.com/BirknerAlex/kubyl/commit/41137a7726724734ff7f5336c059ae55f8b13bb0))
- A rekey during a write no longer leaves the cluster busy ([54d2dce](https://github.com/BirknerAlex/kubyl/commit/54d2dcedb79ef4158fa34f62b00f97ca78df8f01))
- EKS reads describe every update and report describe errors ([d96c025](https://github.com/BirknerAlex/kubyl/commit/d96c02509562c97fe54493d0eba10e5e3c6c0a9a))
- No integer overflow on server-supplied versions ([331f392](https://github.com/BirknerAlex/kubyl/commit/331f392f035899c9a752605cd4adef645ff1fcac))
- CAPI read fails when KubeadmControlPlanes can't be listed ([cb98223](https://github.com/BirknerAlex/kubyl/commit/cb98223e38fc220aa2e1b3fc2c36a1c8e3faee5f))
- OpenShift patch clears a stale desiredUpdate.image ([f30a054](https://github.com/BirknerAlex/kubyl/commit/f30a05476418574d37986143f8d6d643cbd84bc0))
- Cache short-lived cloud credentials for part of their life ([f40a621](https://github.com/BirknerAlex/kubyl/commit/f40a621a16fe095819db20455023cb91c2936f1a))
- Typed confirmation for Scale on PROD from list/palette ([9771bb5](https://github.com/BirknerAlex/kubyl/commit/9771bb5b35198ac3235bc60c7e7fc54ae0273956))
- Mask Secret data in Copy YAML ([d88175b](https://github.com/BirknerAlex/kubyl/commit/d88175bb025fb0e2e86b919e39036f90acc44bf8))
- Include the cluster id in list RowId ([24cfd61](https://github.com/BirknerAlex/kubyl/commit/24cfd61e03fab6786f257e596fd63b74cb1fee78))
- Loopback OAuth callback tolerates idle connections and checks state on errors ([e76f116](https://github.com/BirknerAlex/kubyl/commit/e76f11636bf7da26944ca541ca2f63c1ffc25e97))
- Only drop the OIDC refresh token on invalid_grant ([0fc8f33](https://github.com/BirknerAlex/kubyl/commit/0fc8f336150593c6c2ae45a91e86557ebe27e09e))
- Keep a pending or confirmed scale when the details target changes ([487534a](https://github.com/BirknerAlex/kubyl/commit/487534a126e74c2fcdfb2bb1cf696d9091a5c488))
- Report skipped items when part of a bulk action didn't run ([430204f](https://github.com/BirknerAlex/kubyl/commit/430204f05b4b5b64ac3c749c435396d5f1ef3203))
- Ignore stale kubeconfig reload results ([2a076fe](https://github.com/BirknerAlex/kubyl/commit/2a076fefc67052fde43c7efee6a307f82bdd06c8))
- Plan and start the kubeconfig watcher off the UI thread ([39db6f0](https://github.com/BirknerAlex/kubyl/commit/39db6f0f32f0ae0eb6f377ba809de0b32b43ceb2))
- Repeat RBAC checks after a reconnect or a failed check ([cc93f18](https://github.com/BirknerAlex/kubyl/commit/cc93f18fd2cdea61e1207704c023fc2326b80bf6))
- Keep PROD and read-only flags while a cluster is disconnected ([593ff9f](https://github.com/BirknerAlex/kubyl/commit/593ff9fcdb37740486959ea3efeb5cb585acb76f))
- Reject OpenShift tokens the API server treats as anonymous ([cc73347](https://github.com/BirknerAlex/kubyl/commit/cc73347355c600f3991ff4310f6f0d262149bc33))
- Serialize dev file credential store updates ([71613b8](https://github.com/BirknerAlex/kubyl/commit/71613b8b4b87bf01d2a9e548b17aa1fba72ab7b1))
- Confirm Cordon/Uncordon, typed on PROD ([5c4b040](https://github.com/BirknerAlex/kubyl/commit/5c4b040e41a03606cdc1e31ef3a6dc1af4774731))
- Key alert selection by (cluster, fingerprint) ([9e53a9f](https://github.com/BirknerAlex/kubyl/commit/9e53a9f8a0ae3674eb99b1ca62387b6231c6b951))
- Copied helm commands carry --kubeconfig ([3c75908](https://github.com/BirknerAlex/kubyl/commit/3c7590892f67896aa8f4382a42c462cc85ae59e2))
- Uninstall with CRDs keeps CRDs another CSV owns ([aaadab2](https://github.com/BirknerAlex/kubyl/commit/aaadab2a08faeea89ba1c25e955f8543f18ce838))
- Clamp sync-window duration and bound the walk ([28425ee](https://github.com/BirknerAlex/kubyl/commit/28425eea8b549197775fc7ed4fb56266ff709690))
- Confirm Terminate and switching sync policies on ([2b7d67e](https://github.com/BirknerAlex/kubyl/commit/2b7d67e044ebfb75c1e7e868b803fa1dd8448634))
- Sign-out cancels a sign-in in progress ([acf28cf](https://github.com/BirknerAlex/kubyl/commit/acf28cfb24a6c0c09bd1078abe0d01e9d3d04e52))
- Record the API forward at once and match ids in forward_stopped ([5a10314](https://github.com/BirknerAlex/kubyl/commit/5a103140f468f9e86eb87b6845621698295a2407))
- Iterative glob_match ([c83d504](https://github.com/BirknerAlex/kubyl/commit/c83d504936226609829a1487402acaca4c3ec9cd))
- Only keep http(s) argocd-cm urls ([06bf5c3](https://github.com/BirknerAlex/kubyl/commit/06bf5c3dee51749a0e18089284de97e8d2cbaf93))
- Drop in-flight API requests on connect/disconnect ([c3376a1](https://github.com/BirknerAlex/kubyl/commit/c3376a15e5afb6d96b7b1c83df1a62329e9f35a1))
- Check patch RBAC before a Kubernetes-mode delete and bind it to the UID ([f9c0f66](https://github.com/BirknerAlex/kubyl/commit/f9c0f66359f371ae0037c10f6224223a59fbe843))
- Fill_between can't index past a shorter baseline ([f8d8b6c](https://github.com/BirknerAlex/kubyl/commit/f8d8b6cc248daad320d64c99719c5e946a209c6a))
- Short_repo strips credentials up to the last @ before the path ([60e271f](https://github.com/BirknerAlex/kubyl/commit/60e271f8a32fa717337bb0a878805834ffb491f8))
- Never overwrite an unparsable settings.json ([01e033a](https://github.com/BirknerAlex/kubyl/commit/01e033a2a98a325014db1437bc52e3ab592a524d))
- Don't panic when a settings section is not an object ([ab9fdca](https://github.com/BirknerAlex/kubyl/commit/ab9fdcaa484de445f1d5c80398e748cb5c6840f0))
- Unique temp file and ordered, serialised settings/state writes ([68bdb07](https://github.com/BirknerAlex/kubyl/commit/68bdb0767e2694ae35f7967edaafa2cd66aaf9cd))
- Wait for in-flight settings/state writes on quit ([392b719](https://github.com/BirknerAlex/kubyl/commit/392b7191282b32c5f9c91784c004a366c0a469c2))
- Private config dir when there is no platform config dir ([fa4c23c](https://github.com/BirknerAlex/kubyl/commit/fa4c23ca3630a192c992572a49b4fac950645f24))
- Read the wizard's certificate file off the UI thread ([7a162fd](https://github.com/BirknerAlex/kubyl/commit/7a162fd520f2536991e208a6de797c8a12a36706))
- Exec-plugin consent covers where a relative command points ([037df1d](https://github.com/BirknerAlex/kubyl/commit/037df1dd2e243486bda132ebfc6e13a30da2174f))
- Verify existing Secret and bindings before importing a service-account token ([0a946d1](https://github.com/BirknerAlex/kubyl/commit/0a946d126f6bf95c336e93832e7062e273838830))
- Don't keep every finished load task in the editor ([a8beee0](https://github.com/BirknerAlex/kubyl/commit/a8beee0042501f741d418cb5f865423b6cbe1bd9))
- Preview update channel falls back to stable; manifest channel is checked ([df5fb5e](https://github.com/BirknerAlex/kubyl/commit/df5fb5e83687aa0c9bd88546da70e28482a619ed))
- A panic while applying an update no longer crashes the app ([63c23b2](https://github.com/BirknerAlex/kubyl/commit/63c23b2a3d3ea6975f11d8628323b957d4e9d990))
- Pin the signing identity when verifying a downloaded macOS bundle ([94b07e5](https://github.com/BirknerAlex/kubyl/commit/94b07e51b24512119ed4d095413a51219a15a250))
- Step past a full page of already-sent Loki records ([ca41ba8](https://github.com/BirknerAlex/kubyl/commit/ca41ba857027e72f5a49c68c7a85d2df2ff00a1d))
- Do not request a Loki URL the FlowCollector chose ([132bbe8](https://github.com/BirknerAlex/kubyl/commit/132bbe8e80cf6cbf8c7baaff76aa7f15c37d62fc))
- Park an idle web view on about:blank ([8a36034](https://github.com/BirknerAlex/kubyl/commit/8a36034165a039f40b5860d9ed0a8a8c01672323))
- Re-create the web view when Private changed during creation ([7c89358](https://github.com/BirknerAlex/kubyl/commit/7c8935894b3493e69aa066c7beae1689e9be61e5))
- Do not save accepted certificates in Private mode ([12312c2](https://github.com/BirknerAlex/kubyl/commit/12312c215771cc76cb263771e7ae0bce917e35e6))
- Ask before a page opens the system browser ([785f40b](https://github.com/BirknerAlex/kubyl/commit/785f40b05ed41b752c50e5c5820ce584c9755278))
- Escape quotes and backslashes in LogQL regex values ([f7ae474](https://github.com/BirknerAlex/kubyl/commit/f7ae474cc5ca88dc70d45bcaa09dc6b51552fdc6))
- No unchecked arithmetic on Loki/metrics numbers ([fe3a5b6](https://github.com/BirknerAlex/kubyl/commit/fe3a5b6a851641fdf0616131cad285eb959f3d3d))
- Stop the replaced state's relay forward on Rekeyed ([851ac58](https://github.com/BirknerAlex/kubyl/commit/851ac58c7ecaeae217d1cf394fb915de7b00e059))
- Keep Seen counts exact past 2000 values ([22f1283](https://github.com/BirknerAlex/kubyl/commit/22f128395baf825a445bc2bee9900c6d2fb6fb4f))
- Mask more credential headers ([9a53edd](https://github.com/BirknerAlex/kubyl/commit/9a53edd3b99bb673052b48e572f863a53e23d2e9))
- Parse E/Ei quantity suffixes ([119b1c9](https://github.com/BirknerAlex/kubyl/commit/119b1c9777cd0c7b8803bed58600cf9a2c1a916e))
- Avoid underflow with an oversized recent list ([7ec2f4f](https://github.com/BirknerAlex/kubyl/commit/7ec2f4f82c82218cc1cbbb768148c9ffb3231dac))
- Treat a pod as unlimited when any container lacks a limit ([f74a725](https://github.com/BirknerAlex/kubyl/commit/f74a7259e627b55587304bf95fc0f2b92c37a829))
- Stop port-forwards when their cluster disconnects or is removed ([3ecf677](https://github.com/BirknerAlex/kubyl/commit/3ecf677dd5043f645a62f688b00dbf92c4de2755))
- Gate the write-script test helper to unix ([cbf2158](https://github.com/BirknerAlex/kubyl/commit/cbf2158d2eb15ec0a1920b4ee0d1c35edb1ce85b))
- Treat rooted exec commands as absolute on Windows ([65b57be](https://github.com/BirknerAlex/kubyl/commit/65b57be1929efc4a4701336a95d797035e61d4ba))
- Restart alerts state on cluster rekey ([5a0431f](https://github.com/BirknerAlex/kubyl/commit/5a0431f40d99b9761ed7cb415b3489c609f567aa))
- Bump generation on drop_cluster, stop old forwards on discovery ([16deed4](https://github.com/BirknerAlex/kubyl/commit/16deed4f6d273d26c99f8ce3a94b13d9fc4c35f2))
- Keep previous alerts when every Alertmanager read fails ([cd2cf72](https://github.com/BirknerAlex/kubyl/commit/cd2cf720f185310ac8581018d3741536656ce0d2))
- Do not start a refetch while a read is in flight ([ceba62c](https://github.com/BirknerAlex/kubyl/commit/ceba62c4589e1609c9bbdd352c97a7001f526f95))
- Keep the silences list and report the error when the read fails ([9c38116](https://github.com/BirknerAlex/kubyl/commit/9c381168fee76476e37fc18ad7264a1acbeecab9))
- Skip target check without a namespace, encode path segments ([cc77e4b](https://github.com/BirknerAlex/kubyl/commit/cc77e4b2b8f379c22e4cb303a2fe90185048fe89))
- Expire the PrometheusRule lookup cache ([9b8a8c0](https://github.com/BirknerAlex/kubyl/commit/9b8a8c049b338801815f2d8efa95dabaf3f87354))
- Report failed silence undo, check read-only in recreate_now ([a89817d](https://github.com/BirknerAlex/kubyl/commit/a89817da355c59b391e0bacb36ad7ed7dbda1c3b))
- Cache decoded CSV icons ([1307025](https://github.com/BirknerAlex/kubyl/commit/1307025a730ca753b8977633a65ba639af1e2c1f))
- Ignore uninstall submit while one is running ([3dd5693](https://github.com/BirknerAlex/kubyl/commit/3dd56934d6a694d4db4731c8ae293cd0d6a0d578))
- Only list CRDs the uninstall will delete ([3db69ef](https://github.com/BirknerAlex/kubyl/commit/3db69ef640d381fcdd079bd47812813c1684fcf4))
- Restart an in-flight OperatorHub fetch on rekey ([f7e2e04](https://github.com/BirknerAlex/kubyl/commit/f7e2e0404fa7a6e95bcaf86b1b6a2e22f723891a))
- Refresh Secret originals when only the masked render is unchanged ([9108163](https://github.com/BirknerAlex/kubyl/commit/9108163a260573155bfb50b277c42f4f1d54ac89))
- Cap YAML alias expansion ([4003566](https://github.com/BirknerAlex/kubyl/commit/40035668b3eec6738e99993f7ab63cb651a3259e))
- Cap log line length, fetch size and JSON level parsing ([52fff30](https://github.com/BirknerAlex/kubyl/commit/52fff30be24198a932892082e867d80327718623))
- Bound the gone-pod chips in the logs view ([d49bb99](https://github.com/BirknerAlex/kubyl/commit/d49bb99061323db573ceedc16add032dc3834a39))
- Bracket IPv6 hosts and use the bind address in active() ([7925d52](https://github.com/BirknerAlex/kubyl/commit/7925d526f3ad2b4f5a40e05ec67586b492a74190))
- Bind the one-click forward's port off the UI thread, no probe race ([2b862e4](https://github.com/BirknerAlex/kubyl/commit/2b862e4fd17095c211875eb72c2526de790c4b7e))
- Enforce read-only on saved and auto-start forwards ([775bd0d](https://github.com/BirknerAlex/kubyl/commit/775bd0d7a87021a28abe31d6b24f82315920d693))
- Re-read the client after resolving a workload selector ([43d24ce](https://github.com/BirknerAlex/kubyl/commit/43d24ce10991716635c1232bede06531bb0673a4))
- Pass the new fallback flag in the netflow live test ([defff9d](https://github.com/BirknerAlex/kubyl/commit/defff9df1247f3beff324bdce6ca9a45a35deb65))
- Address review feedback on phase 17 fixes ([e654b2a](https://github.com/BirknerAlex/kubyl/commit/e654b2aa675db8cdb6024633713d942ed784735e))
- Gate the unusable-fallback settings test to unix ([4e03162](https://github.com/BirknerAlex/kubyl/commit/4e031624db9c7a0ad488fd7cda6da159e5d5ede7))

### Documentation

- Add phase 17 code-review fixes checklist ([eb9f538](https://github.com/BirknerAlex/kubyl/commit/eb9f5385b74b40150366bdacf815b9315830b6f0))
- Tick all phase 17 findings and update handoff log ([58f3f5e](https://github.com/BirknerAlex/kubyl/commit/58f3f5e48cf4cdc8ba672e3e06db4955b577cb8a))

### Miscellaneous

- Update Flatpak manifest to v0.3.3 ([af6b722](https://github.com/BirknerAlex/kubyl/commit/af6b722419539d9d0bac8a7e276e3f0503ce2d82))

## [0.3.3] - 2026-09-28

### Miscellaneous

- Update Flatpak manifest to v0.3.2 ([3d52b61](https://github.com/BirknerAlex/kubyl/commit/3d52b6122994c9632c560882b9edac409ec02bbe))

## [0.3.2] - 2026-09-27

### Bug Fixes

- Move Flathub manifest update into release.yml as a job ([b0be571](https://github.com/BirknerAlex/kubyl/commit/b0be571f08ce68728e17bf7b80aadb52e9c9b239))
- Run manifest/artifact fetches on the Tokio runtime (#18) ([3ec09da](https://github.com/BirknerAlex/kubyl/commit/3ec09da3e2932fca6aa5a87b47aa580f9ebd4d30))

### Miscellaneous

- Update Flatpak manifest to v0.3.1 ([3780a0a](https://github.com/BirknerAlex/kubyl/commit/3780a0ab462a956d8ca36cf66e14cccdc45622ce))

## [0.3.1] - 2026-09-27

### Bug Fixes

- Install minisign via apt, not taiki-e/install-action ([0e77618](https://github.com/BirknerAlex/kubyl/commit/0e776184217a166ddb27450e9edc7b0aef4ca447))

## [0.3.0] - 2026-09-27

### Bug Fixes

- Import Cursor for the Linux test module too ([edd0c23](https://github.com/BirknerAlex/kubyl/commit/edd0c23c20cecc0b96e2f00ad33ac0a38527ffee))
- Replace the whole app bundle on macOS, add HTTP timeouts ([fe35140](https://github.com/BirknerAlex/kubyl/commit/fe35140da4d832531d6363787e793ce8210d81e2))
- Publish-flathub committed to a detached HEAD, not main ([e9c1e60](https://github.com/BirknerAlex/kubyl/commit/e9c1e608ba3b8f0e657ae38bea7573eab906b22e))
- Correct binary path, add aarch64 and Metainfo, fix update script ([2bda980](https://github.com/BirknerAlex/kubyl/commit/2bda9806ebd6a97b66664234659321d85564a409))
- Vendor the GTK plugin, fail loudly on a missing WebKit helper ([6d142f8](https://github.com/BirknerAlex/kubyl/commit/6d142f87c42982850d0e2e6b5a3b3c283aad49c8))

### CI

- Prevent Windows CRLF normalization on signed test fixtures ([40f9c7e](https://github.com/BirknerAlex/kubyl/commit/40f9c7e0f3760d402ac5e8ea809dee18586077ab))

### Documentation

- Winget PR submitted, PR #16 merged ([7b58785](https://github.com/BirknerAlex/kubyl/commit/7b5878504dbc0d61f7f0a04e66fbe03bee04d952))

### Features

- Add Linux AppImage packaging ([d128ecd](https://github.com/BirknerAlex/kubyl/commit/d128ecd98d8e6262cc9c110d9e34e0ee88dc33c7))

### Packaging

- Add Flathub manifest and auto-update workflow ([ee23b69](https://github.com/BirknerAlex/kubyl/commit/ee23b69390f81076dc995b06852f2de132a8ecd9))

### Plans

- Update phase 09 with distribution and auto-update status ([cecb0cd](https://github.com/BirknerAlex/kubyl/commit/cecb0cdd91da74cde279b0b594608b71885745a2))
- Kubyl.dev is registered and live ([5fff87f](https://github.com/BirknerAlex/kubyl/commit/5fff87f77b3e9b10ad5b70cb01c005b6a6689598))
- README badges/contributing/license already done ([0337700](https://github.com/BirknerAlex/kubyl/commit/0337700d6ac9f8934ee6146dd0de095f2f33ffb9))
- Log the CodeRabbit review pass and Windows CI fix ([0790223](https://github.com/BirknerAlex/kubyl/commit/079022329900fe9d1196988be6607139b30fb8db))

### Release

- Sign the update manifest and publish to winget and silo ([00420b8](https://github.com/BirknerAlex/kubyl/commit/00420b89d8f475821c6a1d9839c443d7e3946d76))

### Selfupdate

- Add kubyl_selfupdate crate for app auto-update ([d97903f](https://github.com/BirknerAlex/kubyl/commit/d97903f78c8bf41515ef9867fba096b8636639dd))

## [0.2.5] - 2026-09-27

### Plan

- Filter every Flow field in the network flows table, not a subset ([8119e83](https://github.com/BirknerAlex/kubyl/commit/8119e836b394f3e0b7ad9e35d9cc52cfc8fdb90e))

### README

- Features, install, build, contributing, sponsoring ([955c5f7](https://github.com/BirknerAlex/kubyl/commit/955c5f7ceac9d423292cdf6b6bf000d714e72957))
- Sponsoring without tiers, any amount helps ([f17703b](https://github.com/BirknerAlex/kubyl/commit/f17703ba75b34b6188c8ab43da1a8f8c46886c95))

### Design

- Board 18 (network flows: live table with filter chips, completion and flow details; topology at namespace and workload zoom; Calico Whisker and NetObserv without Loki; no flow source and a forbidden port-forward) ([7e350b3](https://github.com/BirknerAlex/kubyl/commit/7e350b3b88e0ea8ae5322d706cbc041160895f2f))
- Phase 16 screenshots of every board 18 frame on the kind clusters ([49af0ec](https://github.com/BirknerAlex/kubyl/commit/49af0ecac6aa1ad91f34d97ec3a56df390db428b))

### Kubyl_charts

- Force-directed graph layout with warm starts and fitting (kubyl_charts::graph, phase 16) ([df47f55](https://github.com/BirknerAlex/kubyl/commit/df47f55fc6f30e056cd66622afa4a833760f495d))
- Layouts that don't depend on node order, and grouped layouts (phase 16) ([732f71d](https://github.com/BirknerAlex/kubyl/commit/732f71d38eb10726f082c50d48bc4806390dba3f))

### Kubyl_explorer

- Namespace views other crates add to the favorites menu (catalog::register_namespace_view, phase 16) ([576c01d](https://github.com/BirknerAlex/kubyl/commit/576c01d52674aefd713879d78e8be12424124b69))

### Kubyl_metrics

- Transport::get_lines streams a response line by line (server-sent events for Calico Whisker, phase 16) ([231b7e3](https://github.com/BirknerAlex/kubyl/commit/231b7e33b8d5f4d28b6519e4dc5946462889024b))
- Transport::loopback for HTTP over a temporary port-forward (phase 16) ([ed0d2ec](https://github.com/BirknerAlex/kubyl/commit/ed0d2ec0b7b43a14c9a596bb64db1b69e42bf3df))

### Kubyl_netflow

- Stub crate (phase 16) with the vendored Hubble API (Cilium v1.20.2 protos, generated by protox in build.rs) ([b44ff51](https://github.com/BirknerAlex/kubyl/commit/b44ff517d857c58f9114f213eb96e78c9a294bb5))
- Flow model, filter language with completion, ring buffer, topology aggregation, detection and the Hubble Relay, Calico Whisker and NetObserv backends (live tests against the netflow-dev.sh clusters) ([6abf794](https://github.com/BirknerAlex/kubyl/commit/6abf7948707864ed379afbeb6f0e88de91e1a23f))
- The Network Flows view, its service, actions and details section (phase 16) ([eb70169](https://github.com/BirknerAlex/kubyl/commit/eb70169ab59a3fbe8eb54a9b11e4801dcf6023dd))
- Whisker over a temporary port-forward; blocking states like board 18 (phase 16) ([5d297f8](https://github.com/BirknerAlex/kubyl/commit/5d297f81ed4944facb52873c32deec9051bcc4b6))
- The view against board 18 on the kind clusters (phase 16) ([5347ce0](https://github.com/BirknerAlex/kubyl/commit/5347ce08f972799413ff88a8b2584feafbd1b0cc))
- Only the blocked count is red in the details section (phase 16) ([2f46f66](https://github.com/BirknerAlex/kubyl/commit/2f46f66ff00f989fe1b3ea2c3daeaebd203bab6a))
- Review fixes (phase 16, review) ([7f98939](https://github.com/BirknerAlex/kubyl/commit/7f98939f79ca5486ef1e4596887ebd6884b38b7d))

### Kubyl_ui

- Waypoints icon (network flows, phase 16) ([1936496](https://github.com/BirknerAlex/kubyl/commit/19364969656e38bee482ece1cb5b7206e3846c4d))

### Plans/16

- Correct the plan after the spike (Whisker's HTTP API instead of Goldmane's mTLS gRPC, Calico 3.30+, Hubble's policy fields, isolation vs deny rules, NetObserv through Loki and its metrics, gRPC through forwards only) ([117eedd](https://github.com/BirknerAlex/kubyl/commit/117eedd10a110e152860d4fcf10edbed9a607cb8))

### Plans/README

- Phase 16 decisions (flow model and providers, detection and overrides, transports, buffer and streaming, filter language, topology layout, sensitive fields, read-only clusters) ([09e12c3](https://github.com/BirknerAlex/kubyl/commit/09e12c30241f6118933a4923cf4056b98941d35d))

### Script

- Netflow-dev.sh (kind clusters with Cilium and Hubble Relay, NetObserv with Loki, Calico with Whisker; payments and storefront traffic, an isolating NetworkPolicy and a deny rule) ([81d43a8](https://github.com/BirknerAlex/kubyl/commit/81d43a8faa8e96d5cb749f932443524cf994c84a))

### Workspace

- Tonic, prost and protox for the Hubble Relay client, fjadra for the topology layout (phase 16) ([b43bf0e](https://github.com/BirknerAlex/kubyl/commit/b43bf0e42c520e2c5f65c4081a26635ebd0d0da9))

### Zed

- Run task uses the file credential store ([8740ee4](https://github.com/BirknerAlex/kubyl/commit/8740ee41333171b55ce3a4aabf920cda62d92ffb))

## [0.2.4] - 2026-09-27

### AGENTS.md

- Phase 13 live tests, cloud provider fixtures and mock endpoints ([1d3017c](https://github.com/BirknerAlex/kubyl/commit/1d3017cad01e2b0efb1308f408d0dd6233b77655))

### CI

- Clippy and test the cloud update providers' features, cargo-deny with all features (phase 13) ([3832633](https://github.com/BirknerAlex/kubyl/commit/3832633523a0c53f51b8c35dee672395bae8b10a))
- Cargo-deny keeps the action's default --all-features (review) ([f31b819](https://github.com/BirknerAlex/kubyl/commit/f31b819a1dd2342c8acc1c18d8c4aa3ad29048cc))

### Design

- Board 8 frames (OpenShift channel, update graph, pre-flight, progress, confirmation, EKS, k3s, read-only, credentials) and the Route frames (Routes under Network, Route web view) ([2c08474](https://github.com/BirknerAlex/kubyl/commit/2c0847486bf77ee6ea2f3e002c04fb05decfab15))
- Phase 13 Routes screenshots ([2cf0f0f](https://github.com/BirknerAlex/kubyl/commit/2cf0f0ff23118b7ba1ee1e701ae7468963ce9fe0))
- Phase 13 screenshots (OpenShift updates, channel, graph, pre-flight, progress, confirmation, EKS, k3s, self-managed, read-only, credentials missing, provider not built, 403, Route details) ([8bf209a](https://github.com/BirknerAlex/kubyl/commit/8bf209a1af37328945bf4481ae085f6b3c7acbda))

### Kubyl

- Forward the cloud update provider features (updates-eks, updates-gke, updates-aks) ([87c3d90](https://github.com/BirknerAlex/kubyl/commit/87c3d90db7b0422f1842eea2cc37f3d553a5c4d8))

### Kubyl_argocd

- Route keys stay masked in live/desired diffs (phase 13) ([bedca86](https://github.com/BirknerAlex/kubyl/commit/bedca86f3c416b5de22c65ec96a252b0cfc12034))
- The TLS key note covers a stale last-applied copy (review) ([7044cd7](https://github.com/BirknerAlex/kubyl/commit/7044cd797075f2852ca3ceff994d3c6930eeb9fa))

### Kubyl_explorer

- Routes under Network with their own details (phase 13) ([27b2c50](https://github.com/BirknerAlex/kubyl/commit/27b2c50c55029dda54ab777bd005566c2b8eb151))
- Route details resolve numeric target ports through the endpoints (phase 13) ([3910027](https://github.com/BirknerAlex/kubyl/commit/39100278258a4f5e4f3a53f0983b88dbdefc2e53))

### Kubyl_metrics

- Read monitoring Routes through the shared Route model (phase 13) ([2bc41db](https://github.com/BirknerAlex/kubyl/commit/2bc41dbe8cf6ddd2100e766e020a1cfc2481faa9))

### Kubyl_operators

- Mask inline Route keys in Helm manifests (phase 13) ([c036b4a](https://github.com/BirknerAlex/kubyl/commit/c036b4aa0b200dd1daf8343a7c887fb0e9bfb570))

### Kubyl_palette

- Route and Service references (phase 13) ([1d47bc0](https://github.com/BirknerAlex/kubyl/commit/1d47bc07407092ab3066d61940740c0ce7bec21c))

### Kubyl_portforward

- Forward a Route's or Ingress's backend Service (phase 13) ([f3266e1](https://github.com/BirknerAlex/kubyl/commit/f3266e19ac15eb041330b6585a7467c4ca2f57e6))
- Route forwards read the backend's EndpointSlices (phase 13) ([188c1f9](https://github.com/BirknerAlex/kubyl/commit/188c1f9c86f5bbcf3966fa72aa3147141d8f2875))
- The Route live test takes namespaces without Ingresses (real clusters) ([44ed669](https://github.com/BirknerAlex/kubyl/commit/44ed669a23aa79d6945379392e1f3349b0989304))
- Backend forwards match the API group as well as the resource (review) ([342a17c](https://github.com/BirknerAlex/kubyl/commit/342a17c4ee706146f2ecaa5861056afe7ec03cf5))
- The Route live test fails when a backend doesn't resolve (review) ([8d4a895](https://github.com/BirknerAlex/kubyl/commit/8d4a8950d8724b03f20efcfb42beb8ab3d6626a8))

### Kubyl_resources

- Route model, columns and target port resolution (phase 13) ([27c0d35](https://github.com/BirknerAlex/kubyl/commit/27c0d35b823b74c3b8bf38f69fea70e216bd37c4))
- Resolve numeric Route target ports through the endpoints (phase 13) ([6dc7a3b](https://github.com/BirknerAlex/kubyl/commit/6dc7a3b7d9c67ae5198115cff4eb4cf18377e405))
- A Route's named target port reports the number its endpoints serve (phase 13) ([b6122dc](https://github.com/BirknerAlex/kubyl/commit/b6122dcc0e19debff00ef3e0a4c783ee0f968a69))
- Wider Services and Admitted columns for Routes (phase 13) ([07893ee](https://github.com/BirknerAlex/kubyl/commit/07893ee3c8f07a21367a916ffa20af17ea862309))
- Mask a Route's last-applied copy when it still holds a key taken out of spec (review) ([8856f0c](https://github.com/BirknerAlex/kubyl/commit/8856f0c6cb732e0b06402a8fa717835f3a44a691))

### Kubyl_ui

- Route icon (OpenShift Routes, phase 13) ([67e5912](https://github.com/BirknerAlex/kubyl/commit/67e59124e70a8e444437f9e02217bbe0080ead16))

### Kubyl_updates

- Provider trait, detection, model, versions, removed-API table, settings ([bf19337](https://github.com/BirknerAlex/kubyl/commit/bf19337c01f64cf66bba01c6a22d2606cbbe0a4f))
- OpenShift, k3s/RKE2 (system-upgrade-controller), Cluster API and read-only providers, pre-flight checks, the update service and the Cluster Updates tab ([0dcec4a](https://github.com/BirknerAlex/kubyl/commit/0dcec4a46a1f236c5724d0ffb4776e49fbe193d2))
- Provider override in settings, live tests against the dev clusters ([26bcdd6](https://github.com/BirknerAlex/kubyl/commit/26bcdd6cdd1281473cfac9d51b4c9669515c38fd))
- Shared cloud provider plumbing (CLIs, HTTP, errors, exec plugin, credential cache) ([64ed5d5](https://github.com/BirknerAlex/kubyl/commit/64ed5d551460e84949def110ed91b3fc6d7e01c3))
- Amazon EKS update provider (updates-eks) ([12dc54f](https://github.com/BirknerAlex/kubyl/commit/12dc54fc3b2b2146193eef533bdd4bcad4a17075))
- Cloud API endpoint overrides (KUBYL_UPDATES_{EKS,GKE,AKS}_ENDPOINT) ([962b53f](https://github.com/BirknerAlex/kubyl/commit/962b53fb17f132338275ee02d4dd5e843c281fb8))
- Google GKE update provider (updates-gke) ([21881ae](https://github.com/BirknerAlex/kubyl/commit/21881aef941eaeb119dc9307a0f86858d0f8ed77))
- Azure AKS update provider (updates-aks) ([e09175a](https://github.com/BirknerAlex/kubyl/commit/e09175ae87b2b3e99c5aec4ab6d1b371cfc178a7))
- The read-only OpenShift live test runs the pre-flight checks too ([3f2b4d2](https://github.com/BirknerAlex/kubyl/commit/3f2b4d2474bbd16a256998a451fe274bcda5b951))
- Operators and pools first while an update runs, wait for Prometheus discovery before the checks, node pools follow the control plane, write hints only where the provider can write, wrapping and graph fixes ([f667089](https://github.com/BirknerAlex/kubyl/commit/f667089084b402ce1d8e374ac274219ea21bfb6c))
- Live test that updates the k3d cluster through its Plans and follows it ([11f2571](https://github.com/BirknerAlex/kubyl/commit/11f25715c5a68fe05d5eb0613d845955c9b0bb8a))
- One recorded-response bundle per cloud (tests/fixtures/{eks,gke,aks}.json) ([3f1ec7d](https://github.com/BirknerAlex/kubyl/commit/3f1ec7d78233f2e7f243e22e5e8739d949baba53))
- Review fixes: the last-applied scan picks the kind's own group and never passes on an incomplete scan, pre-releases sort before their release, a pre-flight run without inputs finishes, one note for failed EKS add-on versions, AKS pools target only newer versions ([75892bf](https://github.com/BirknerAlex/kubyl/commit/75892bf628cc40052be734658a562f6c32f02d47))
- Pre-release stages order before their numbers (ec < fc < rc) (review) ([f2873b9](https://github.com/BirknerAlex/kubyl/commit/f2873b9b88362487fc7effb7fcdacf9ebc0b34dd))

### Kubyl_webview

- Web views of Route backends (phase 13) ([659df65](https://github.com/BirknerAlex/kubyl/commit/659df65225faa68fff3e079f20ec170086fc4bd2))
- Route web views resolve numeric target ports through the endpoints (phase 13) ([a6a502b](https://github.com/BirknerAlex/kubyl/commit/a6a502bda97e170ea41d214f4fc15e8272c847d6))

### Kubyl_yaml

- Route template and inline TLS key masking (phase 13) ([330b578](https://github.com/BirknerAlex/kubyl/commit/330b578d0c912de9c9b3f7c278f120010de44be8))
- Stale Route keys in the last-applied copy stay masked in the editor, manifests and apply history (review) ([50aab37](https://github.com/BirknerAlex/kubyl/commit/50aab375f2e04dd64933e061134220d8bfbc3119))

### Plans/12

- OpenShift check done on the user's test cluster ([3525e80](https://github.com/BirknerAlex/kubyl/commit/3525e8035dfd3998562db2111dd663478f8ffbcc))

### Plans/13

- Track the OpenShift Routes work (added on request) ([8d93ec1](https://github.com/BirknerAlex/kubyl/commit/8d93ec16ad7aa4286b57d689b45a67db2457176e))
- Done, handoff log ([a65773a](https://github.com/BirknerAlex/kubyl/commit/a65773a541d321c22e1115d227751c4f3a2e2c9c))
- Test counts ([35db59b](https://github.com/BirknerAlex/kubyl/commit/35db59b9a7b12c5e385131dccd46c731701e4170))

### Plans/README

- Phase 13 decisions (update providers, writes, removed and deprecated APIs, cloud credentials, features, Routes) ([42c17c0](https://github.com/BirknerAlex/kubyl/commit/42c17c08777f54e3b68eed61ee31e0316394d7f3))
- Phase 13 extension points (update state, provider trait, Route helpers) and the cloud providers' details ([0d3180c](https://github.com/BirknerAlex/kubyl/commit/0d3180cafc9ea9cf5c2b27dd8f820cf82092f20e))

### Script

- Updates-dev.sh (PDB and removed-API Helm release on kind, a fake OpenShift cluster with ClusterVersion, operators, pools, APIRequestCounts and Routes, a k3s cluster with system-upgrade-controller) ([54236b6](https://github.com/BirknerAlex/kubyl/commit/54236b62fa7f50ac5fa70141ab35196f5fed3418))

### Workspace

- Hmac for SigV4 signing of the EKS update provider (phase 13) ([5e7b7ca](https://github.com/BirknerAlex/kubyl/commit/5e7b7ca1a1780a7c8362d3c9f914ab754fc24a5a))

## [0.2.3] - 2026-09-26

### Bug Fixes

- Data protection keychain for release builds, no more keychain prompts ([3362bb0](https://github.com/BirknerAlex/kubyl/commit/3362bb07e5eb13e566ad681f3b47f9859243757f))

### Design

- Phase 12 screenshots (Operators, OperatorHub, upgrade review, Helm releases, states) ([66b9c32](https://github.com/BirknerAlex/kubyl/commit/66b9c32bca4b59cfe5ce6c8593af4f73227ef687))

### Kubyl_explorer

- Helm Releases row under Administration, not gated on OLM (phase 12) ([2951c8f](https://github.com/BirknerAlex/kubyl/commit/2951c8f25af23b860e4ae723b1ee8b1041fdf0eb))

### Kubyl_operators

- OLM and Helm data layer ([b2d3718](https://github.com/BirknerAlex/kubyl/commit/b2d3718b22ffd017c580d0fa7481a27a30008182))
- Services, views and dialogs ([e27e4bd](https://github.com/BirknerAlex/kubyl/commit/e27e4bde6a7716ac70fe3d0acd2b3e42f608b9cd))
- Live tests; icons, plain-text descriptions, transient ResolutionFailed, review order, dialog focus ([ff2b54d](https://github.com/BirknerAlex/kubyl/commit/ff2b54dba6c78cab68af40077da60e1ca3561a84))
- Release tabs wait for their cluster, OLM v1 template without installer accounts, 5,000-row test; olm-dev.sh --reset-manual and v1 with any cert-manager namespace; docs ([a74cb12](https://github.com/BirknerAlex/kubyl/commit/a74cb12866a89fd3fd107881d80ad4a4e342035b))
- OLM v1 condition tones and Retrying, tab title follows the sub-tab, no list hints on blocking states, plain-word RBAC hints; olm-dev.sh v1 sample grafana-operator ([d814250](https://github.com/BirknerAlex/kubyl/commit/d814250b3115c3716c2ab5181cc9352c6d831f34))
- Live tests point at olm-dev.sh --reset-manual ([af01a14](https://github.com/BirknerAlex/kubyl/commit/af01a1441ffa9ef5092868aea9d5fd1112f73fb1))
- Loading until discovery is known; History loads revisions as they arrive (review) ([65c0472](https://github.com/BirknerAlex/kubyl/commit/65c04725c3acf554fab2dd713f31fe50f91343a5))
- Only Subscription and CSV watches block api::installed; Helm watch errors before the scope; OperatorHub's installed filter follows subscriptions; v1 banner matches the template (review) ([fadb0d0](https://github.com/BirknerAlex/kubyl/commit/fadb0d05a175c8a822122212eeddac6f0afe8a69))

### Kubyl_ui

- Anchor icon (Helm releases, phase 12) ([fcd7c34](https://github.com/BirknerAlex/kubyl/commit/fcd7c34504ed74f6248c977ee9bce392eaafa3fa))

### Kubyl_yaml

- Open_draft opens a new-resource editor with given text and a note (for phase 12's operator examples) ([0152b96](https://github.com/BirknerAlex/kubyl/commit/0152b96acd713fbd3d49edcf983f367c7ec3015e))

### Mockups

- Board 7 frames for OperatorHub, install, upgrade review, alm-examples, install plans, subscriptions, Helm releases and the OLM states ([2373161](https://github.com/BirknerAlex/kubyl/commit/2373161b2a720bb6842c21d7cdb7516d06de15db))

### Olm-dev.sh

- --delete removes only a cert-manager --v1 installed, and operator-controller's CA where it went; plans: PROD confirmation per write (review) ([4d9ee19](https://github.com/BirknerAlex/kubyl/commit/4d9ee199940e996a1706bd177b0dd61a7e0ece3c))
- --delete removes only what the script installed (marked namespaces); waits for a running cert-manager's webhook and retries operator-controller's apply (review) ([ad06856](https://github.com/BirknerAlex/kubyl/commit/ad06856c805f1eefeaa00133e07da99fe68643d4))

### Plans

- Phase 12 decisions (OLM data and joins, upgrade review sources, OperatorHub, install/uninstall, OLM v1, Helm, views) ([2236c0e](https://github.com/BirknerAlex/kubyl/commit/2236c0e4c747858dd640bb62b5a73d4ae50c7c1a))
- Phase 12 done (checkboxes, handoff log; Operators views and OLM v1 rows) ([566557f](https://github.com/BirknerAlex/kubyl/commit/566557f816c9988fbb7b0dcf1c8db977bd16d289))

### Release.yml

- Sign with the certificate the provisioning profile names, fail on an expired profile (review) ([50fb698](https://github.com/BirknerAlex/kubyl/commit/50fb698555a70c602c82384e735d40184b27a2f9))

### Workspace

- Flate2 for gzip-encoded Helm releases and OLM bundles (phase 12) ([182ab96](https://github.com/BirknerAlex/kubyl/commit/182ab963a4911ad7369d2f5fec340b1959c97b51))

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


