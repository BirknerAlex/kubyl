#!/usr/bin/env python3
"""Fake OpenShift update state for the kind cluster `kubyl-ocp` of script/updates-dev.sh.

Nothing of OpenShift runs there: this writes what the cluster-version operator, the cluster
operators and the machine-config operator would report, through the status subresources.

  fake_ocp.py --kubeconfig F --context kind-kubyl-ocp setup
      ClusterVersion (created once, with a random clusterID), ClusterOperators,
      MachineConfigPools, APIRequestCounts and the admin-gates/admin-acks ConfigMaps, then the
      idle stage.
  ... stage <idle|started|operators|nodes|done>
      A fixed state: idle (4.17.8), started (12%), operators (61%), nodes (a worker drains) or
      done (4.17.12). Clears the admin acks and sets spec.channel/spec.desiredUpdate.
  ... update [--stage-seconds N]
      idle, then the stages towards 4.17.12, N seconds apart.
  ... cvo [--stage-seconds N]
      A fake cluster-version operator in the foreground (Ctrl-C stops). Every 2 s it reads
      spec.channel, spec.desiredUpdate and openshift-config/admin-acks: it recomputes the
      available and conditional updates for the channel, and plays the stages towards a new
      desiredUpdate if its preconditions pass (the version is recommended, or its risks are all
      named in spec.desiredUpdate.acceptRisks, or force is set; a minor update also needs the
      admin gate acked). A refused update shows as ReleaseAccepted=False. An update that a
      fixed stage left half-way is finished first.

Every kubectl call passes --kubeconfig and --context, the context must be kind-kubyl-ocp, and
nothing is written unless that cluster's kube-system namespace carries the annotation
kubyl.dev/installed-by=updates-dev.sh.
"""

import argparse
import base64
import datetime as dt
import hashlib
import json
import re
import signal
import subprocess
import sys
import time
import uuid
from concurrent.futures import ThreadPoolExecutor

CONTEXT = "kind-kubyl-ocp"
MARK_KEY = "kubyl.dev/installed-by"
MARK = "updates-dev.sh"
STAGE_KEY = "kubyl.dev/fake-cvo-stage"
FIELD_MANAGER = "updates-dev.sh"

BASE_VERSION = "4.17.8"
TARGET_VERSION = "4.17.12"
BASE_CHANNEL = "stable-4.17"
UPDATE_STAGES = ("started", "operators", "nodes")
STAGES = ("idle",) + UPDATE_STAGES + ("done",)
TOTAL_MANIFESTS = 845
# (manifests done, percent) per stage, as the CVO words it.
STAGE_PROGRESS = {"started": (106, 12), "operators": (512, 61), "nodes": (752, 89)}
# How long ago a fixed stage's update started (minutes).
STARTED_AGO = {"started": 2, "operators": 21, "nodes": 43}

ADMIN_GATE_KEY = "ack-4.17-kube-1.31-api-removals-in-4.18"
ADMIN_GATE_MESSAGE = (
    "Kubernetes 1.31 and therefore OpenShift 4.18 remove several APIs which require admin "
    "consideration. Please see the knowledge article https://access.redhat.com/articles/0000000 "
    "for details and instructions."
)

# The releases the fake update service knows. Errata numbers count from 4.17.8 (RHBA-2026:0001).
KNOWN = [
    "4.15.30", "4.16.10", "4.16.24", "4.17.6", "4.17.8", "4.17.9", "4.17.10", "4.17.11",
    "4.17.12", "4.17.13", "4.17.14", "4.18.2", "4.18.3", "4.18.4",
]
ERRATA = {v: i for i, v in enumerate(KNOWN[KNOWN.index("4.17.8"):], start=1)}

STORAGE_RISK = {
    "name": "ExampleStorageDriverRegression",
    "message": "Clusters using the example CSI driver may see volume attachments time out after "
               "updating to 4.17.13, until the driver is restarted.",
    "url": "https://issues.example.com/OCPBUGS-12345",
    "matchingRules": [{"type": "Always"}],
}
NETPOL_RISK = {
    "name": "ExampleNetworkPolicyChange",
    "message": "OpenShift 4.18 evaluates NetworkPolicies with an empty namespaceSelector "
               "differently; clusters with such policies outside openshift-* namespaces may drop "
               "traffic after the update.",
    "url": "https://issues.example.com/OCPBUGS-23456",
    "matchingRules": [{
        "type": "PromQL",
        "promql": {"promql": 'group(kube_networkpolicy_spec_ingress_namespace_selector_empty'
                             '{namespace!~"openshift-.*"}) or 0 * group(kube_networkpolicy_labels)'},
    }],
}

# The fake update graph: the releases of each channel (None = recommended, else its risks).
# Updates are offered to every newer release of the channel, at most one minor ahead. Risks are
# per channel here (Cincinnati's are per edge), so switching channels shows a difference:
# fast-4.17 recommends 4.17.13, stable-4.18 recommends 4.18.2.
Z_4_17 = ["4.17.6", "4.17.8", "4.17.9", "4.17.10", "4.17.11", "4.17.12"]
CHANNELS = {
    "stable-4.17": {"4.17.13": [STORAGE_RISK], "4.18.2": [NETPOL_RISK]},
    "fast-4.17": {"4.17.13": None, "4.18.2": [NETPOL_RISK]},
    "candidate-4.17": {"4.17.13": None, "4.17.14": None, "4.18.2": [NETPOL_RISK]},
    "stable-4.18": {"4.17.13": [STORAGE_RISK], "4.18.2": None},
    "eus-4.18": {"4.17.13": [STORAGE_RISK], "4.18.2": None},
    "fast-4.18": {"4.17.13": None, "4.18.2": None, "4.18.3": None},
    "candidate-4.18": {"4.17.13": None, "4.17.14": None, "4.18.2": None, "4.18.3": None,
                       "4.18.4": None},
}

# Roughly in the order the CVO's runlevels update them.
OPERATORS = [
    "config-operator", "etcd", "kube-apiserver", "kube-controller-manager", "kube-scheduler",
    "kube-storage-version-migrator", "cloud-credential", "cloud-controller-manager",
    "openshift-apiserver", "openshift-controller-manager", "authentication", "service-ca",
    "machine-api", "control-plane-machine-set", "cluster-autoscaler", "machine-approver",
    "baremetal", "csi-snapshot-controller", "storage", "image-registry", "ingress",
    "node-tuning", "operator-lifecycle-manager", "operator-lifecycle-manager-catalog",
    "operator-lifecycle-manager-packageserver", "network", "dns", "machine-config",
    "marketplace", "monitoring", "console", "insights", "openshift-samples",
]
UPDATED_AT_OPERATORS_STAGE = 20  # the first 20 are at the target version at 61%

CAPABILITIES = [
    "Build", "CSISnapshot", "CloudControllerManager", "CloudCredential", "Console",
    "DeploymentConfig", "ImageRegistry", "Ingress", "Insights", "MachineAPI", "NodeTuning",
    "OperatorLifecycleManager", "Storage", "baremetal", "marketplace", "openshift-samples",
]


def die(message):
    print(f"\033[1;31merror:\033[0m {message}", file=sys.stderr)
    sys.exit(1)


def log(message):
    print(f"\033[1;34m==>\033[0m {message}", flush=True)


def now_utc():
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0)


def iso(t):
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


def ver(v):
    return tuple(int(x) for x in v.split("."))


def image(v):
    digest = hashlib.sha256(f"kubyl-fake-ocp-release-{v}".encode()).hexdigest()
    return f"quay.io/openshift-release-dev/ocp-release@sha256:{digest}"


def channels(v):
    _, minor, _ = ver(v)
    cur, nxt = f"4.{minor}", f"4.{minor + 1}"
    out = [f"candidate-{cur}", f"fast-{cur}", f"stable-{cur}"]
    if minor % 2 == 0:
        out.append(f"eus-{cur}")
    out += [f"candidate-{nxt}", f"fast-{nxt}", f"stable-{nxt}"]
    if (minor + 1) % 2 == 0:
        out.append(f"eus-{nxt}")
    return out


def release(v):
    r = {"version": v, "image": image(v)}
    if v in ERRATA:
        r["url"] = f"https://access.redhat.com/errata/RHBA-2026:{ERRATA[v]:04d}"
    r["channels"] = channels(v)
    return r


def version_for_image(img):
    return next((v for v in KNOWN if image(v) == img), None)


def rendered(pool, v):
    return f"rendered-{pool}-{hashlib.md5(f'{pool}-{v}'.encode()).hexdigest()}"


def kube_version(v):
    _, minor, z = ver(v)
    return f"1.{minor + 13}.{z + 2}"


def version_hash(v):
    return base64.urlsafe_b64encode(hashlib.sha256(v.encode()).digest()[:8]).decode()


class KubeError(Exception):
    pass


class Kube:
    """kubectl with an explicit --kubeconfig and --context on every call."""

    def __init__(self, kubeconfig, context):
        self.kubeconfig = kubeconfig
        self.context = context

    def run(self, *args, stdin=None):
        cmd = ["kubectl", "--kubeconfig", self.kubeconfig, "--context", self.context, *args]
        p = subprocess.run(cmd, input=stdin, capture_output=True, text=True)
        if p.returncode != 0:
            raise KubeError(p.stderr.strip() or f"kubectl {' '.join(args[:2])} failed")
        return p.stdout

    def json(self, *args):
        return json.loads(self.run(*args, "-o", "json"))

    def json_or_none(self, *args):
        try:
            return self.json(*args)
        except KubeError as e:
            if "NotFound" in str(e) or "not found" in str(e):
                return None
            raise

    def apply(self, items):
        self.run("apply", "--server-side", "--force-conflicts", f"--field-manager={FIELD_MANAGER}",
                 "-f", "-", stdin=json.dumps({"apiVersion": "v1", "kind": "List", "items": items}))

    def merge(self, resource, name, patch, namespace=None):
        ns = ["-n", namespace] if namespace else []
        self.run("patch", resource, name, *ns, "--type=merge", "-p", json.dumps(patch))

    def set_status(self, resource, name, status, namespace=None):
        ns = ["-n", namespace] if namespace else []
        op = [{"op": "add", "path": "/status", "value": status}]
        self.run("patch", resource, name, *ns, "--subresource=status", "--type=json",
                 "-p", json.dumps(op))


def guard(kube):
    if kube.context != CONTEXT:
        die(f"only context {CONTEXT} is supported (got {kube.context})")
    try:
        ns = kube.json("get", "namespace", "kube-system")
    except KubeError as e:
        die(f"context {kube.context} in {kube.kubeconfig} isn't reachable: {e}")
    if (ns["metadata"].get("annotations") or {}).get(MARK_KEY) != MARK:
        die(f"context {kube.context} isn't the fake OpenShift cluster of script/updates-dev.sh "
            f"(kube-system lacks {MARK_KEY}={MARK}); not touching it")


def run_parallel(jobs):
    with ThreadPoolExecutor(max_workers=8) as pool:
        for future in [pool.submit(job) for job in jobs]:
            future.result()


def merge_conditions(old, new, now):
    """Keeps lastTransitionTime of conditions whose status didn't change."""
    prev = {c.get("type"): c for c in old or []}
    out = []
    for c in new:
        c = dict(c)
        if "lastTransitionTime" not in c:
            o = prev.get(c["type"])
            same = o and o.get("status") == c["status"] and o.get("lastTransitionTime")
            c["lastTransitionTime"] = o["lastTransitionTime"] if same else iso(now)
        out.append(c)
    return out


# ----- The world: what version the cluster runs and what update is in progress -----

def completed(v, started, finished, verified=True):
    return {"state": "Completed", "startedTime": iso(started), "completionTime": iso(finished),
            "version": v, "image": image(v), "verified": verified}


def base_history(now):
    # version, started (days ago), duration (minutes); the last entry is the installation.
    rows = [("4.17.8", 12, 74), ("4.17.6", 41, 81), ("4.16.24", 63, 96), ("4.16.10", 118, 88),
            ("4.15.30", 190, 43)]
    day = now.replace(hour=9, minute=12, second=0)
    out = []
    for i, (v, days, minutes) in enumerate(rows):
        started = day - dt.timedelta(days=days)
        out.append(completed(v, started, started + dt.timedelta(minutes=minutes),
                             verified=i < len(rows) - 1))
    return out


def fixed_world(stage, now):
    world = {"version": BASE_VERSION, "history": base_history(now), "update": None,
             "acked": False, "refusal": None}
    if stage in UPDATE_STAGES:
        started = now - dt.timedelta(minutes=STARTED_AGO[stage])
        world["update"] = {"from": BASE_VERSION, "to": TARGET_VERSION, "stage": stage,
                           "started": iso(started), "acceptedRisks": None}
    elif stage == "done":
        started = now - dt.timedelta(minutes=62)
        world["version"] = TARGET_VERSION
        world["history"].insert(0, completed(TARGET_VERSION, started, now - dt.timedelta(minutes=1)))
    return world


def world_from_cluster(cv, acks):
    status = cv.get("status") or {}
    history = list(status.get("history") or [])
    desired = (status.get("desired") or {}).get("version") or BASE_VERSION
    stage = ((cv["metadata"].get("annotations") or {}).get(STAGE_KEY)) or "idle"
    acked = ((acks or {}).get("data") or {}).get(ADMIN_GATE_KEY) == "true"
    world = {"version": desired, "history": history, "update": None, "acked": acked,
             "refusal": None}
    if history and history[0].get("state") == "Partial" and stage in UPDATE_STAGES:
        top = history.pop(0)
        frm = history[0]["version"] if history else desired
        world["version"] = frm
        world["update"] = {"from": frm, "to": top["version"], "stage": stage,
                           "started": top["startedTime"], "acceptedRisks": top.get("acceptedRisks")}
    return world


def offers(current, channel, now):
    """(available, conditional, RetrievedUpdates condition) from `current` in `channel`."""
    if not channel:
        return [], [], {"type": "RetrievedUpdates", "status": "False", "reason": "NoChannel",
                        "message": "The update channel has not been configured."}
    extra = CHANNELS.get(channel)
    releases = None if extra is None else {**{v: None for v in Z_4_17}, **extra}
    if releases is None or current not in releases:
        return [], [], {
            "type": "RetrievedUpdates", "status": "False", "reason": "VersionNotFound",
            "message": f"Unable to retrieve available updates: currently reconciling cluster "
                       f'version {current} not found in the "{channel}" channel'}
    since = iso(now.replace(hour=0, minute=0, second=0))
    available, conditional = [], []
    for v in sorted(releases, key=ver):
        if ver(v) <= ver(current) or ver(v)[1] > ver(current)[1] + 1:
            continue
        risks = releases[v]
        if risks is None:
            available.append(release(v))
            continue
        conditional.append({
            "release": release(v),
            "risks": risks,
            "conditions": [{
                "type": "Recommended", "status": "False",
                "reason": risks[0]["name"] if len(risks) == 1 else "MultipleReasons",
                "message": "\n\n".join(f"{r['message']} {r['url']}" for r in risks),
                "lastTransitionTime": since,
            }],
        })
    return available, conditional, {"type": "RetrievedUpdates", "status": "True"}


def cv_status(world, cv, now):
    up = world["update"]
    applied = world["version"]
    target = up["to"] if up else applied
    channel = (cv.get("spec") or {}).get("channel") or ""
    available, conditional, retrieved = offers(target, channel, now)
    history = list(world["history"])
    if up:
        entry = {"state": "Partial", "startedTime": up["started"], "completionTime": None,
                 "version": up["to"], "image": image(up["to"]), "verified": True}
        if up.get("acceptedRisks"):
            entry["acceptedRisks"] = up["acceptedRisks"]
        history.insert(0, entry)
    if world.get("refusal"):
        reason, message = world["refusal"]
        accepted = {"type": "ReleaseAccepted", "status": "False", "reason": reason,
                    "message": message}
    else:
        accepted = {"type": "ReleaseAccepted", "status": "True", "reason": "PayloadLoaded",
                    "message": f'Payload loaded version="{target}" image="{image(target)}" '
                               f'architecture="amd64"'}
    if up:
        done, percent = STAGE_PROGRESS[up["stage"]]
        message = (f"Working towards {up['to']}: {done} of {TOTAL_MANIFESTS} done "
                   f"({percent}% complete)")
        progressing = {"type": "Progressing", "status": "True", "message": message}
        if up["stage"] != "started":
            progressing["message"] += ", waiting on machine-config"
            progressing["reason"] = "ClusterOperatorUpdating"
    else:
        progressing = {"type": "Progressing", "status": "False",
                       "message": f"Cluster version is {applied}"}
    conditions = [
        retrieved,
        {"type": "ImplicitlyEnabledCapabilities", "status": "False", "reason": "AsExpected",
         "message": "Capabilities match configured spec"},
        accepted,
        {"type": "Available", "status": "True", "message": f"Done applying {applied}"},
        {"type": "Failing", "status": "False"},
        progressing,
    ]
    if admin_gate_applies(applied) and not world["acked"]:
        conditions.append({"type": "Upgradeable", "status": "False", "reason": "AdminAckRequired",
                           "message": ADMIN_GATE_MESSAGE})
    old = cv.get("status") or {}
    status = {
        "availableUpdates": available or None,
        "capabilities": {"enabledCapabilities": CAPABILITIES, "knownCapabilities": CAPABILITIES},
        "conditions": merge_conditions(old.get("conditions"), conditions, now),
        "desired": release(target),
        "history": history,
        "observedGeneration": cv["metadata"].get("generation", 1),
        "versionHash": version_hash(target),
    }
    if conditional:
        status["conditionalUpdates"] = conditional
    return status


def admin_gate_applies(applied):
    gate = re.match(r"ack-(\d+\.\d+)-", ADMIN_GATE_KEY).group(1)
    return ".".join(applied.split(".")[:2]) == gate


def co_state(world):
    """Operator name -> (version, None or (reason, Progressing message))."""
    up = world["update"]
    if not up:
        return {n: (world["version"], None) for n in OPERATORS}
    frm, to, stage = up["from"], up["to"], up["stage"]
    if stage == "started":
        state = {n: (frm, None) for n in OPERATORS}
        state["config-operator"] = (to, None)
        state["etcd"] = (frm, ("NodeInstaller", "NodeInstallerProgressing: 1 node is at "
                                                "revision 14; 0 nodes have achieved new revision 15"))
    elif stage == "operators":
        state = {n: (to if i < UPDATED_AT_OPERATORS_STAGE else frm, None)
                 for i, n in enumerate(OPERATORS)}
        state["network"] = (frm, ("Deploying", 'DaemonSet "/openshift-multus/multus" is not '
                                               "available (awaiting 1 nodes)"))
        state["dns"] = (frm, ("DNSReportsProgressingIsTrue", 'DNS "default" reports '
                                                             'Progressing=True: "Have 2 available DNS pods, want 3."'))
        state["machine-config"] = (frm, ("", f"Working towards {to}"))
    else:  # nodes
        state = {n: (to, None) for n in OPERATORS}
        state["machine-config"] = (frm, ("", f"Working towards {to}"))
    return state


def co_status(name, version, progressing, old, now):
    ok = {"status": "False", "reason": "AsExpected", "message": "All is well"}
    if progressing:
        reason, message = progressing
        prog = {"type": "Progressing", "status": "True", "message": message}
        if reason:
            prog["reason"] = reason
    else:
        prog = {"type": "Progressing", **ok}
    conditions = [
        {"type": "Available", **ok, "status": "True"},
        prog,
        {"type": "Degraded", **ok},
        {"type": "Upgradeable", **ok, "status": "True"},
    ]
    versions = [{"name": "operator", "version": version}]
    if name in ("kube-apiserver", "kube-controller-manager", "kube-scheduler"):
        versions += [{"name": "raw-internal", "version": version},
                     {"name": name, "version": kube_version(version)}]
    return {
        "conditions": merge_conditions(old.get("conditions"), conditions, now),
        "versions": versions,
        "relatedObjects": [{"group": "", "resource": "namespaces", "name": f"openshift-{name}"}],
        "extension": None,
    }


def node_roles(kube):
    nodes = kube.json("get", "nodes")["items"]
    masters = sorted(n["metadata"]["name"] for n in nodes
                     if "node-role.kubernetes.io/control-plane" in n["metadata"].get("labels", {}))
    workers = sorted(n["metadata"]["name"] for n in nodes
                     if n["metadata"]["name"] not in masters)
    return nodes, masters, workers


def machine_state(world, masters, workers):
    """(pools, nodes): pool -> (spec config, status config, machines, updated),
    node -> (pool, current config, desired config, state, cordoned)."""
    up = world["update"]
    if not up or up["stage"] in ("started", "operators"):
        v = up["from"] if up else world["version"]
        pools = {"master": (rendered("master", v), rendered("master", v), len(masters), len(masters)),
                 "worker": (rendered("worker", v), rendered("worker", v), len(workers), len(workers))}
        nodes = {n: ("master", rendered("master", v), rendered("master", v), "Done", False)
                 for n in masters}
        nodes.update({n: ("worker", rendered("worker", v), rendered("worker", v), "Done", False)
                      for n in workers})
        return pools, nodes
    frm, to = up["from"], up["to"]
    new_m, new_w, old_w = rendered("master", to), rendered("worker", to), rendered("worker", frm)
    pools = {"master": (new_m, new_m, len(masters), len(masters)),
             "worker": (new_w, old_w, len(workers), min(1, len(workers)))}
    nodes = {n: ("master", new_m, new_m, "Done", False) for n in masters}
    for i, n in enumerate(workers):
        if i == 0:
            nodes[n] = ("worker", new_w, new_w, "Done", False)
        elif i == 1:
            nodes[n] = ("worker", old_w, new_w, "Working", True)  # draining
        else:
            nodes[n] = ("worker", old_w, old_w, "Done", False)
    return pools, nodes


def mcp_sources(pool):
    names = [f"00-{pool}", f"01-{pool}-container-runtime", f"01-{pool}-kubelet",
             f"97-{pool}-generated-kubelet", f"98-{pool}-generated-kubelet",
             f"99-{pool}-generated-registries", f"99-{pool}-ssh"]
    return [{"apiVersion": "machineconfiguration.openshift.io/v1", "kind": "MachineConfig",
             "name": n} for n in names]


def mcp_status(pool, spec_cfg, status_cfg, machines, updated, old, generation, now):
    updating = spec_cfg != status_cfg
    ready = updated if updating else machines
    f = {"status": "False", "reason": "", "message": ""}
    conditions = [
        {"type": "RenderDegraded", **f},
        {"type": "NodeDegraded", **f},
        {"type": "Degraded", **f},
        {"type": "Updated", **f, "status": "False" if updating else "True",
         "message": "" if updating else f"All nodes are updated with MachineConfig {status_cfg}"},
        {"type": "Updating", **f, "status": "True" if updating else "False",
         "message": f"All nodes are updating to MachineConfig {spec_cfg}" if updating else ""},
    ]
    return {
        "observedGeneration": generation,
        "configuration": {"name": status_cfg, "source": mcp_sources(pool)},
        "machineCount": machines,
        "readyMachineCount": ready,
        "updatedMachineCount": updated,
        "unavailableMachineCount": machines - ready,
        "degradedMachineCount": 0,
        "conditions": merge_conditions(old.get("conditions"), conditions, now),
        "certExpirys": [{"bundle": "KubeAPIServerServingCAData",
                         "subject": "CN=admin-kubeconfig-signer,OU=openshift",
                         "expiry": "2035-03-01T09:12:00Z"}],
    }


def apply_world(kube, world, label, spec=None):
    """Writes the world: ClusterVersion, ClusterOperators, MachineConfigPools and nodes."""
    now = now_utc()
    patch = {"metadata": {"annotations": {STAGE_KEY: label}}}
    if spec is not None:
        patch["spec"] = spec
    kube.merge("clusterversion", "version", patch)
    cv = kube.json("get", "clusterversion", "version")
    kube.set_status("clusterversion", "version", cv_status(world, cv, now))

    jobs = []
    cos = {c["metadata"]["name"]: c for c in kube.json("get", "clusteroperators")["items"]}
    for name, (version, progressing) in co_state(world).items():
        old = (cos.get(name) or {}).get("status") or {}
        new = co_status(name, version, progressing, old, now)
        if new != old:
            jobs.append(lambda n=name, s=new: kube.set_status("clusteroperator", n, s))

    nodes, masters, workers = node_roles(kube)
    pools, node_states = machine_state(world, masters, workers)
    mcps = {m["metadata"]["name"]: m for m in kube.json("get", "machineconfigpools")["items"]}
    for pool, (spec_cfg, status_cfg, machines, updated) in pools.items():
        mcp = mcps.get(pool)
        if not mcp:
            continue
        if ((mcp.get("spec") or {}).get("configuration") or {}).get("name") != spec_cfg:
            kube.merge("machineconfigpool", pool,
                       {"spec": {"configuration": {"name": spec_cfg, "source": mcp_sources(pool)}}})
            mcp = kube.json("get", "machineconfigpool", pool)
        old = mcp.get("status") or {}
        new = mcp_status(pool, spec_cfg, status_cfg, machines, updated, old,
                         mcp["metadata"].get("generation", 1), now)
        if new != old:
            jobs.append(lambda p=pool, s=new: kube.set_status("machineconfigpool", p, s))

    by_name = {n["metadata"]["name"]: n for n in nodes}
    prefix = "machineconfiguration.openshift.io/"
    for name, (pool, current, desired, state, cordoned) in node_states.items():
        drain = f"drain-{desired}" if state == "Working" else f"uncordon-{current}"
        applied_drain = f"uncordon-{current}" if state == "Working" else drain
        annotations = {prefix + "currentConfig": current, prefix + "desiredConfig": desired,
                       prefix + "state": state, prefix + "reason": "",
                       prefix + "desiredDrain": drain, prefix + "lastAppliedDrain": applied_drain}
        node = by_name[name]
        have = node["metadata"].get("annotations") or {}
        if (any(have.get(k) != v for k, v in annotations.items())
                or bool(node["spec"].get("unschedulable")) != cordoned):
            patch = {"metadata": {"annotations": annotations},
                     "spec": {"unschedulable": True if cordoned else None}}
            jobs.append(lambda n=name, p=patch: kube.merge("node", n, p))
    run_parallel(jobs)


def describe(world):
    up = world["update"]
    if not up:
        return f"at {world['version']}"
    return f"{up['from']} -> {up['to']}: {up['stage']}"


# ----- Subcommands -----

def spec_for(target, channel=BASE_CHANNEL):
    # A merge patch: null removes acceptRisks/architecture a previous test may have set.
    return {"channel": channel,
            "desiredUpdate": {"version": target, "image": image(target), "force": False,
                              "acceptRisks": None, "architecture": None}}


def clear_acks(kube):
    kube.merge("configmap", "admin-acks", {"data": None}, namespace="openshift-config")


def stage(kube, name):
    now = now_utc()
    world = fixed_world(name, now)
    clear_acks(kube)
    target = world["update"]["to"] if world["update"] else world["version"]
    apply_world(kube, world, name, spec=spec_for(target))
    log(f"kubyl-ocp: stage {name} ({describe(world)})")


def play(kube, world, to, seconds, accepted_risks=None, start="started", set_spec=False):
    """Walks an update through the stages towards `to`, `seconds` apart."""
    frm = world["version"]
    started = world["update"]["started"] if world["update"] else iso(now_utc())
    for st in UPDATE_STAGES[UPDATE_STAGES.index(start):]:
        world["update"] = {"from": frm, "to": to, "stage": st, "started": started,
                           "acceptedRisks": accepted_risks}
        spec = spec_for(to) if set_spec and st == "started" else None
        apply_world(kube, world, st, spec=spec)
        log(f"{frm} -> {to}: {st} ({STAGE_PROGRESS[st][1]}%)")
        time.sleep(seconds)
    entry = {"state": "Completed", "startedTime": started, "completionTime": iso(now_utc()),
             "version": to, "image": image(to), "verified": True}
    if accepted_risks:
        entry["acceptedRisks"] = accepted_risks
    world["history"].insert(0, entry)
    world["version"] = to
    world["update"] = None
    apply_world(kube, world, "done")
    log(f"{frm} -> {to}: done")


def update(kube, seconds):
    stage(kube, "idle")
    time.sleep(seconds)
    world = fixed_world("idle", now_utc())
    play(kube, world, TARGET_VERSION, seconds, set_spec=True)


def preconditions(world, cv, desired):
    """(version, acceptedRisks, refusal) for spec.desiredUpdate; version None = nothing to do."""
    status = cv.get("status") or {}
    current = world["version"]
    img = desired.get("image") or ""
    target = desired.get("version") or version_for_image(img)
    if not target:
        if not img:
            return None, None, None
        return None, None, ("RetrievePayload",
                            f'Retrieving payload failed version="" image="{img}" failure=The '
                            f"update cannot be verified: the fake update service doesn't know "
                            f"this release image")
    if target == current:
        return None, None, None
    if target not in KNOWN:
        return None, None, ("VersionNotFound",
                            f"Version {target} isn't in the available or conditional updates "
                            f"(the fake update service knows {', '.join(KNOWN[KNOWN.index(BASE_VERSION):])})")
    payload = f'Preconditions failed for payload loaded version="{target}" image="{image(target)}"'
    force = bool(desired.get("force"))
    failures = []
    if ver(target)[1] != ver(current)[1] and admin_gate_applies(current) and not world["acked"]:
        failures.append(f'Precondition "ClusterVersionUpgradeable" failed because of '
                        f'"AdminAckRequired": {ADMIN_GATE_MESSAGE}')
    available = {u["version"] for u in status.get("availableUpdates") or []}
    conditional = {c["release"]["version"]: c for c in status.get("conditionalUpdates") or []}
    accepted = None
    if target not in available:
        if target in conditional:
            risks = [r["name"] for r in conditional[target]["risks"]]
            acked = {a.get("name") for a in desired.get("acceptRisks") or []}
            if set(risks) <= acked:
                accepted = (f"The target release {target} is exposed to the risks "
                            f"[{', '.join(risks)}] and accepted by CVO because all of them are "
                            f"considered acceptable")
            else:
                reason = risks[0] if len(risks) == 1 else "MultipleReasons"
                message = conditional[target]["conditions"][0]["message"]
                failures.append(f'Precondition "ClusterVersionRecommendedUpdate" failed because '
                                f'of "{reason}": Update from {current} to {target} is not '
                                f"recommended:\n\n{message}")
        else:
            failures.append(f'Precondition "ClusterVersionRecommendedUpdate" failed because of '
                            f'"UnknownUpdate": {target} is not in the available or conditional '
                            f"updates of this channel")
    if not failures:
        return target, accepted, None
    summary = failures[0] if len(failures) == 1 else (
        "Multiple precondition checks failed:\n* " + "\n* ".join(failures))
    if force:
        return target, f"Forced through blocking failures: {summary}", None
    return None, None, ("PreconditionChecks", f"{payload}: {summary}")


def cvo(kube, seconds):
    log(f"fake CVO on {CONTEXT}: following clusterversion/version every 2 s "
        f"(stages {seconds} s apart; Ctrl-C stops)")
    last = None
    while True:
        try:
            cv = kube.json("get", "clusterversion", "version")
            acks = kube.json_or_none("get", "configmap", "-n", "openshift-config", "admin-acks")
            world = world_from_cluster(cv, acks)
            if world["update"]:
                up = world["update"]
                nxt = UPDATE_STAGES[min(UPDATE_STAGES.index(up["stage"]) + 1,
                                        len(UPDATE_STAGES) - 1)]
                log(f"finishing the update to {up['to']} (at {up['stage']})")
                play(kube, world, up["to"], seconds, up.get("acceptedRisks"), start=nxt)
                continue
            desired = (cv.get("spec") or {}).get("desiredUpdate") or {}
            target, accepted, refusal = preconditions(world, cv, desired)
            if target:
                log(f"spec.desiredUpdate asks for {target}: updating from {world['version']}"
                    + (" (risks accepted)" if accepted else ""))
                play(kube, world, target, seconds, accepted)
                continue
            world["refusal"] = refusal
            status = cv_status(world, cv, now_utc())
            if status != (cv.get("status") or {}):
                kube.set_status("clusterversion", "version", status)
            channel = (cv.get("spec") or {}).get("channel") or ""
            summary = (channel, len(status.get("availableUpdates") or []),
                       len(status.get("conditionalUpdates") or []), refusal, world["acked"])
            if summary != last:
                log(f"{world['version']} on channel {channel or '(none)'}: {summary[1]} available, "
                    f"{summary[2]} conditional"
                    + ("; admin gate acked" if world["acked"] else "")
                    + (f"; refused: {refusal[1].splitlines()[0]}" if refusal else ""))
                last = summary
        except KubeError as e:
            log(f"kubectl failed ({e}); retrying")
        time.sleep(2)


def request_counts(masters, now):
    """APIRequestCount objects with status, like the kube-apiserver's."""
    node = masters[0] if masters else "master-0"
    kubectl = "kubectl/v1.30.4 (darwin/arm64) kubernetes/55f2bc2"
    auditor = ("system:serviceaccount:legacy-tools:flow-auditor",
               "flow-auditor/v0.9.1 (linux/amd64) kubernetes/4f3a1c2")
    specs = [
        ("flowschemas.v1beta3.flowcontrol.apiserver.k8s.io", "1.32",
         [(*auditor, {"list": 18, "watch": 6, "get": 4}),
          ("system:admin", kubectl, {"get": 2})]),
        ("prioritylevelconfigurations.v1beta3.flowcontrol.apiserver.k8s.io", "1.32",
         [(*auditor, {"list": 6, "watch": 2})]),
        ("horizontalpodautoscalers.v2beta2.autoscaling", "1.26",
         [("system:serviceaccount:legacy-app:legacy-app-scaler",
           "legacy-scaler/1.2.0 (linux/amd64) kubernetes/v1.24.0", {"get": 12, "update": 3})]),
        ("deployments.v1.apps", None,
         [("system:serviceaccount:kube-system:deployment-controller",
           "kube-controller-manager/v1.30.10 (linux/amd64) kubernetes/1b2c3d4/"
           "system:serviceaccount:kube-system:deployment-controller",
           {"get": 120, "update": 40, "list": 3, "watch": 3}),
          ("system:admin", kubectl, {"get": 14, "list": 6})]),
        ("routes.v1.route.openshift.io", None,
         [("system:serviceaccount:openshift-ingress:router",
           "openshift-router/v4.17.0 (linux/amd64) kubernetes/0000000",
           {"list": 1, "watch": 6}),
          ("system:admin", kubectl, {"get": 5, "list": 2})]),
    ]
    items, statuses = [], {}
    for name, removed, users in specs:
        last24h = []
        for hour in range(24):
            factor = (hour * 7 + len(name)) % 4  # 0..3, some hours without requests
            by_user = []
            for username, agent, verbs in users:
                by_verb = [{"verb": v, "requestCount": c * factor} for v, c in verbs.items()
                           if c * factor]
                total = sum(b["requestCount"] for b in by_verb)
                if total:
                    by_user.append({"username": username, "userAgent": agent,
                                    "requestCount": total, "byVerb": by_verb})
            total = sum(u["requestCount"] for u in by_user)
            entry = {"requestCount": total}
            if total:
                entry["byNode"] = [{"nodeName": node, "requestCount": total, "byUser": by_user}]
            last24h.append(entry)
        status = {"currentHour": last24h[now.hour],
                  "last24h": last24h,
                  "requestCount": sum(e["requestCount"] for e in last24h)}
        if removed:
            status["removedInRelease"] = removed
        items.append({"apiVersion": "apiserver.openshift.io/v1", "kind": "APIRequestCount",
                      "metadata": {"name": name}, "spec": {"numberOfUsersToReport": 10}})
        statuses[name] = status
    return items, statuses


def setup(kube):
    now = now_utc()
    _, masters, _ = node_roles(kube)
    kube.apply([
        {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": "openshift-config"}},
        {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": "openshift-config-managed"}},
        {"apiVersion": "v1", "kind": "ConfigMap",
         "metadata": {"name": "admin-gates", "namespace": "openshift-config-managed"},
         "data": {ADMIN_GATE_KEY: ADMIN_GATE_MESSAGE}},
    ])
    if not kube.json_or_none("get", "configmap", "-n", "openshift-config", "admin-acks"):
        kube.run("create", "configmap", "admin-acks", "-n", "openshift-config")
    if not kube.json_or_none("get", "clusterversion", "version"):
        cv = {"apiVersion": "config.openshift.io/v1", "kind": "ClusterVersion",
              "metadata": {"name": "version"},
              "spec": {"clusterID": str(uuid.uuid4()), "channel": BASE_CHANNEL}}
        kube.run("create", "-f", "-", stdin=json.dumps(cv))
    kube.apply([{"apiVersion": "config.openshift.io/v1", "kind": "ClusterOperator",
                 "metadata": {"name": n, "annotations": {
                     "include.release.openshift.io/self-managed-high-availability": "true"}},
                 "spec": {}} for n in OPERATORS])
    pools = []
    for pool in ("master", "worker"):
        labels = {f"pools.operator.machineconfiguration.openshift.io/{pool}": "",
                  "machineconfiguration.openshift.io/mco-built-in": ""}
        if pool == "master":
            labels["operator.machineconfiguration.openshift.io/required-for-upgrade"] = ""
        pools.append({
            "apiVersion": "machineconfiguration.openshift.io/v1", "kind": "MachineConfigPool",
            "metadata": {"name": pool, "labels": labels},
            "spec": {"machineConfigSelector": {
                         "matchLabels": {"machineconfiguration.openshift.io/role": pool}},
                     "nodeSelector": {"matchLabels": {f"node-role.kubernetes.io/{pool}": ""}},
                     "paused": False},
        })
    kube.apply(pools)
    items, statuses = request_counts(masters, now)
    kube.apply(items)
    run_parallel([lambda n=n, s=s: kube.set_status("apirequestcount", n, s)
                  for n, s in statuses.items()])
    stage(kube, "idle")


def stop(signum, frame):
    raise KeyboardInterrupt


def main():
    # Also when started in the background, where SIGINT may be ignored.
    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--kubeconfig", required=True)
    parser.add_argument("--context", default=CONTEXT)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("setup")
    st = sub.add_parser("stage")
    st.add_argument("name", choices=STAGES)
    for name in ("update", "cvo"):
        p = sub.add_parser(name)
        p.add_argument("--stage-seconds", type=float, default=15)
    args = parser.parse_args()

    kube = Kube(args.kubeconfig, args.context)
    guard(kube)
    try:
        if args.command == "setup":
            setup(kube)
        elif args.command == "stage":
            stage(kube, args.name)
        elif args.command == "update":
            update(kube, args.stage_seconds)
        else:
            cvo(kube, args.stage_seconds)
    except KubeError as e:
        die(str(e))
    except KeyboardInterrupt:
        print()
        log("stopped")


if __name__ == "__main__":
    main()
