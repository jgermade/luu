//! `kubernetes`: a session's worker as a pod, started with `kubectl run -i`.
//!
//! Still an argv, the shape this layer's first paragraph names: `kubectl run
//! --rm -i` is `docker run --rm -i` in another spelling, and the worker speaks
//! over its stdin and stdout either way. What is new is the pod itself, which
//! is written whole here as the `--overrides` JSON — the work volume by
//! `subPath`, so only the session's folder is in it; a home and a `/tmp` of its
//! own; luu's uid; the chart's own hardening; luu's pod as its owner; and on
//! luu's node, where a `ReadWriteOnce` claim can be mounted.
//!
//! Configured by the environment, because half of it — luu's pod's name, uid
//! and namespace — exists only once that pod does, from the Downward API the
//! chart writes it with. See `RECORD/2026-11-03.a-pod-per-session.WIP.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// What a pod needs that a container does not, read from `LUU_KUBE_*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kube {
    /// The program: `kubectl`, or `LUU_KUBECTL`.
    pub kubectl: String,
    /// Where the workers are made: `LUU_KUBE_NAMESPACE`, else `kubectl`'s own
    /// (in a pod, the pod's).
    pub namespace: Option<String>,
    /// The claim that holds the work tree: `LUU_KUBE_CLAIM`. Required.
    pub claim: String,
    /// Where luu's own pod mounts that claim: `LUU_KUBE_CLAIM_ROOT`, `/work`.
    /// A session's base is mounted as its path under this.
    pub claim_root: PathBuf,
    /// luu's pod, as `ownerReferences` names it: `LUU_KUBE_OWNER_NAME` and
    /// `LUU_KUBE_OWNER_UID`. A worker of a luu that is replaced goes with it.
    pub owner: Option<(String, String)>,
    /// luu's pod's labels, for a required `podAffinity`: `LUU_KUBE_SAME_NODE`,
    /// `a=b,c=d`. Set where the claim is `ReadWriteOnce`, which any number of
    /// pods can mount on one node and none on another.
    pub same_node: Option<BTreeMap<String, String>>,
    /// Labels every worker carries, `LUU_KUBE_LABELS`: what the chart's
    /// NetworkPolicies select by.
    pub labels: BTreeMap<String, String>,
    /// `LUU_KUBE_RUNTIME_CLASS`: gVisor or Kata, where a cluster has them.
    pub runtime_class: Option<String>,
    /// `LUU_KUBE_RESOURCES`: the container's `resources`, as JSON.
    pub resources: Option<Value>,
    /// `LUU_KUBE_PULL_POLICY`.
    pub pull_policy: Option<String>,
    /// luu's own `CARGO_HOME`, carried into the pod, where it is under the
    /// pod's own home and so made per session.
    pub cargo_home: Option<PathBuf>,
}

/// How long `kubectl run` waits for the pod to be running: the first start on
/// a node pulls the worker image.
const RUNNING_TIMEOUT: &str = "5m";

/// The container's command: wait for one line, make the cargo home a granted
/// path needs to exist, then become the worker. **The line is the start**:
/// `kubectl run -i` attaches once the pod runs, and what the container wrote
/// before that is not replayed — so a worker that greeted at once would greet
/// nobody. luu writes the line as soon as `kubectl` is spawned; it sits in
/// `kubectl`'s stdin until the attach carries it, and by then the hello has a
/// reader. See [`super::Worker`]'s start.
const WAIT: &str =
    r#"read -r _ && { [ -z "${CARGO_HOME:-}" ] || mkdir -p "$CARGO_HOME"; } && exec "$@""#;

impl Kube {
    /// From this process's environment.
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
    }

    /// From any lookup — the environment's, or a test's.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let claim = get("LUU_KUBE_CLAIM").ok_or(
            "the kubernetes runtime needs LUU_KUBE_CLAIM: the claim that holds the work tree, \
             which a worker pod mounts its session's folder from. The chart sets it with \
             `runtime: kubernetes`",
        )?;
        let owner = match (get("LUU_KUBE_OWNER_NAME"), get("LUU_KUBE_OWNER_UID")) {
            (Some(name), Some(uid)) => Some((name, uid)),
            (None, None) => None,
            _ => {
                return Err(
                    "LUU_KUBE_OWNER_NAME and LUU_KUBE_OWNER_UID are set together or \
                            not at all"
                        .to_string(),
                );
            }
        };
        let resources = match get("LUU_KUBE_RESOURCES") {
            None => None,
            Some(text) => Some(
                serde_json::from_str::<Value>(&text)
                    .map_err(|error| format!("LUU_KUBE_RESOURCES is not JSON: {error}"))?,
            )
            .filter(|value| !value.as_object().is_some_and(|map| map.is_empty())),
        };
        Ok(Self {
            kubectl: get("LUU_KUBECTL").unwrap_or_else(|| "kubectl".to_string()),
            namespace: get("LUU_KUBE_NAMESPACE"),
            claim,
            claim_root: get("LUU_KUBE_CLAIM_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/work")),
            owner,
            same_node: get("LUU_KUBE_SAME_NODE")
                .map(|text| labels_of(&text, "LUU_KUBE_SAME_NODE"))
                .transpose()?,
            labels: get("LUU_KUBE_LABELS")
                .map(|text| labels_of(&text, "LUU_KUBE_LABELS"))
                .transpose()?
                .unwrap_or_default(),
            runtime_class: get("LUU_KUBE_RUNTIME_CLASS"),
            resources,
            pull_policy: get("LUU_KUBE_PULL_POLICY"),
            cargo_home: get("CARGO_HOME").map(PathBuf::from),
        })
    }

    /// `kubectl` and, where one is named, `--namespace`: what every line here
    /// starts with.
    fn prefix(&self, verb: &str) -> Vec<String> {
        let mut argv = vec![self.kubectl.clone(), verb.to_string()];
        if let Some(namespace) = &self.namespace {
            argv.extend(["--namespace".to_string(), namespace.clone()]);
        }
        argv
    }

    /// The line that starts a worker pod named `name`.
    pub fn run_argv(&self, pod: &Pod<'_>) -> Result<Vec<String>, String> {
        let overrides = self.pod(pod)?;
        let mut argv = self.prefix("run");
        argv.extend([
            pod.name.to_string(),
            "--rm".to_string(),
            "-i".to_string(),
            "--quiet".to_string(),
            "--restart=Never".to_string(),
            format!("--pod-running-timeout={RUNNING_TIMEOUT}"),
            format!("--image={}", pod.image),
            "--override-type=merge".to_string(),
            format!("--overrides={overrides}"),
        ]);
        Ok(argv)
    }

    /// `kubectl exec`, before what it runs.
    pub fn exec_prefix(&self) -> Vec<String> {
        self.prefix("exec")
    }

    /// The pod, whole: what `--overrides` replaces `kubectl run`'s own with.
    pub fn pod(&self, pod: &Pod<'_>) -> Result<Value, String> {
        let base = pod.base;
        let sub = base.strip_prefix(&self.claim_root).map_err(|_| {
            format!(
                "{} is not under {}, where the work claim is mounted: a worker pod can only \
                 mount a folder of the claim",
                base.display(),
                self.claim_root.display()
            )
        })?;
        let base_text = base.display().to_string();

        let mut work = json!({ "name": "work", "mountPath": base_text });
        if !sub.as_os_str().is_empty() {
            work["subPath"] = json!(sub.display().to_string());
        }
        let mut mounts = vec![work, json!({ "name": "tmp", "mountPath": "/tmp" })];
        let mut volumes = vec![
            json!({ "name": "work", "persistentVolumeClaim": { "claimName": self.claim } }),
            json!({ "name": "tmp", "emptyDir": {} }),
        ];
        let mut env = Vec::new();
        if let Some(home) = pod.home {
            env.push(json!({ "name": "HOME", "value": home.display().to_string() }));
            // A home of its own, unless it holds the base: then it is the
            // claim's, and mounting over it would hide the work tree.
            if !base.starts_with(home) {
                mounts.push(json!({ "name": "home", "mountPath": home.display().to_string() }));
                volumes.push(json!({ "name": "home", "emptyDir": {} }));
                if let Some(cargo) = self
                    .cargo_home
                    .as_ref()
                    .filter(|cargo| cargo.starts_with(home))
                {
                    env.push(json!({ "name": "CARGO_HOME", "value": cargo.display().to_string() }));
                }
            }
        }

        let mut labels: BTreeMap<String, String> = self.labels.clone();
        labels.insert("app.kubernetes.io/component".into(), "worker".into());
        labels.insert(
            "luu.dev/network".into(),
            match pod.network {
                true => "open",
                false => "none",
            }
            .into(),
        );

        let mut security = json!({
            "runAsNonRoot": true,
            "seccompProfile": { "type": "RuntimeDefault" },
        });
        if let Some((uid, gid)) = pod.user {
            security["runAsUser"] = json!(uid);
            security["runAsGroup"] = json!(gid);
        }

        let mut container = json!({
            "name": pod.name,
            "image": pod.image,
            "stdin": true,
            "stdinOnce": true,
            "tty": false,
            "command": ["sh", "-c", WAIT, "luu-worker"],
            "args": pod.args,
            "workingDir": base_text,
            "env": env,
            "volumeMounts": mounts,
            "securityContext": {
                "allowPrivilegeEscalation": false,
                "readOnlyRootFilesystem": true,
                "capabilities": { "drop": ["ALL"] },
            },
        });
        if let Some(policy) = &self.pull_policy {
            container["imagePullPolicy"] = json!(policy);
        }
        if let Some(resources) = &self.resources {
            container["resources"] = resources.clone();
        }

        let mut spec = json!({
            "restartPolicy": "Never",
            "automountServiceAccountToken": false,
            "enableServiceLinks": false,
            "securityContext": security,
            "containers": [container],
            "volumes": volumes,
        });
        if let Some(class) = &self.runtime_class {
            spec["runtimeClassName"] = json!(class);
        }
        if let Some(same) = &self.same_node {
            spec["affinity"] = json!({
                "podAffinity": {
                    "requiredDuringSchedulingIgnoredDuringExecution": [{
                        "labelSelector": { "matchLabels": same },
                        "topologyKey": "kubernetes.io/hostname",
                    }],
                },
            });
        }

        let mut metadata = json!({ "labels": labels });
        // Its controller, too: a Deployment's ReplicaSet adopts any pod that
        // matches its selector and has no controller, and then deletes the
        // extra one. The chart's worker labels do not match it either.
        if let Some((name, uid)) = &self.owner {
            metadata["ownerReferences"] = json!([{
                "apiVersion": "v1",
                "kind": "Pod",
                "name": name,
                "uid": uid,
                "controller": true,
            }]);
        }
        Ok(json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": metadata,
            "spec": spec,
        }))
    }
}

/// One worker pod, as the session asks for it.
pub struct Pod<'a> {
    pub name: &'a str,
    pub image: &'a str,
    pub base: &'a Path,
    pub home: Option<&'a Path>,
    pub user: Option<(u32, u32)>,
    pub network: bool,
    /// What the container runs after it has waited: `luu worker …`.
    pub args: Vec<String>,
}

/// `a=b,c=d`, as a selector's labels. Nothing else: no `!=`, no sets.
fn labels_of(text: &str, name: &str) -> Result<BTreeMap<String, String>, String> {
    text.split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) if !key.trim().is_empty() => {
                Ok((key.trim().to_string(), value.trim().to_string()))
            }
            _ => Err(format!("{name}: `{pair}` is not `label=value`")),
        })
        .collect()
}

/// A pod name nothing else in the namespace is using: the time and a count,
/// rather than the container name's process id — in a pod that is 1 every
/// time, and a pod a dead `kubectl` left behind keeps its name until luu's own
/// pod is replaced.
pub fn pod_name() -> String {
    static STARTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("luu-worker-{ms}-{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kube(vars: &[(&str, &str)]) -> Result<Kube, String> {
        let vars: BTreeMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Kube::from_lookup(|name| vars.get(name).cloned())
    }

    fn chart() -> Kube {
        kube(&[
            ("LUU_KUBE_CLAIM", "luu-work"),
            ("LUU_KUBE_NAMESPACE", "luu"),
            ("LUU_KUBE_OWNER_NAME", "luu-7d9f-abc"),
            ("LUU_KUBE_OWNER_UID", "1234-uid"),
            (
                "LUU_KUBE_SAME_NODE",
                "app.kubernetes.io/instance=luu, app.kubernetes.io/component=server",
            ),
            (
                "LUU_KUBE_LABELS",
                "app.kubernetes.io/name=luu-worker,app.kubernetes.io/instance=luu",
            ),
            ("LUU_KUBE_RUNTIME_CLASS", "gvisor"),
            (
                "LUU_KUBE_RESOURCES",
                r#"{"limits":{"cpu":"2","memory":"4Gi"}}"#,
            ),
            ("CARGO_HOME", "/state/cargo"),
        ])
        .expect("the chart's environment")
    }

    fn pod<'a>(base: &'a Path, network: bool) -> Pod<'a> {
        Pod {
            name: "luu-worker-1-0",
            image: "ghcr.io/jgermade/luu-worker:0.1.6",
            base,
            home: Some(Path::new("/state")),
            user: Some((10001, 10001)),
            network,
            args: vec![
                "luu".into(),
                "worker".into(),
                "--command".into(),
                "cargo".into(),
            ],
        }
    }

    #[test]
    fn the_claim_is_required_and_everything_else_has_a_default() {
        let error = kube(&[]).unwrap_err();
        assert!(error.contains("LUU_KUBE_CLAIM"), "{error}");
        let bare = kube(&[("LUU_KUBE_CLAIM", "work")]).unwrap();
        assert_eq!(bare.kubectl, "kubectl");
        assert_eq!(bare.claim_root, PathBuf::from("/work"));
        assert_eq!(bare.namespace, None);
        assert!(kube(&[("LUU_KUBE_CLAIM", "w"), ("LUU_KUBE_OWNER_NAME", "x")]).is_err());
        assert!(kube(&[("LUU_KUBE_CLAIM", "w"), ("LUU_KUBE_LABELS", "nolabel")]).is_err());
        assert!(kube(&[("LUU_KUBE_CLAIM", "w"), ("LUU_KUBE_RESOURCES", "{")]).is_err());
    }

    #[test]
    fn only_the_sessions_folder_is_mounted_and_where_it_lives() {
        let base = Path::new("/work/a1b2c3d4e5f6/project");
        let made = chart().pod(&pod(base, false)).unwrap();
        let container = &made["spec"]["containers"][0];
        let work = &container["volumeMounts"][0];
        // Its own absolute path, the rule every runtime keeps, and the claim's
        // folder by subPath: another user's folder is not in this pod at all.
        assert_eq!(work["mountPath"], "/work/a1b2c3d4e5f6/project");
        assert_eq!(work["subPath"], "a1b2c3d4e5f6/project");
        assert_eq!(container["workingDir"], "/work/a1b2c3d4e5f6/project");
        assert_eq!(
            made["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"],
            "luu-work"
        );
        // A home and a /tmp of its own, and the cargo home under it.
        let mounts = container["volumeMounts"].to_string();
        assert!(mounts.contains(r#""mountPath":"/state""#), "{mounts}");
        assert!(mounts.contains(r#""mountPath":"/tmp""#), "{mounts}");
        assert!(container["env"].to_string().contains("/state/cargo"));
        // A base outside the claim cannot be mounted from it.
        let error = chart()
            .pod(&pod(Path::new("/elsewhere"), false))
            .unwrap_err();
        assert!(error.contains("not under /work"), "{error}");
    }

    #[test]
    fn the_pod_is_held_like_the_charts_own_and_owned_by_luus() {
        let pod = chart().pod(&pod(Path::new("/work/u/p"), false)).unwrap();
        let spec = &pod["spec"];
        assert_eq!(spec["automountServiceAccountToken"], false);
        assert_eq!(spec["securityContext"]["runAsNonRoot"], true);
        assert_eq!(spec["securityContext"]["runAsUser"], 10001);
        assert_eq!(
            spec["securityContext"]["seccompProfile"]["type"],
            "RuntimeDefault"
        );
        assert_eq!(spec["runtimeClassName"], "gvisor");
        let container = &spec["containers"][0];
        assert_eq!(container["securityContext"]["readOnlyRootFilesystem"], true);
        assert_eq!(
            container["securityContext"]["allowPrivilegeEscalation"],
            false
        );
        assert_eq!(
            container["securityContext"]["capabilities"]["drop"][0],
            "ALL"
        );
        assert_eq!(container["resources"]["limits"]["memory"], "4Gi");
        assert_eq!(container["stdin"], true);
        assert_eq!(container["stdinOnce"], true);
        // It waits for the line before it becomes the worker.
        assert_eq!(container["command"][0], "sh");
        assert!(
            container["command"][2]
                .as_str()
                .unwrap()
                .starts_with("read -r _")
        );
        assert_eq!(container["args"][1], "worker");
        let owner = &pod["metadata"]["ownerReferences"][0];
        assert_eq!(owner["kind"], "Pod");
        assert_eq!(owner["uid"], "1234-uid");
        assert_eq!(owner["controller"], true);
        // On luu's node: the claim is ReadWriteOnce.
        let affinity =
            &spec["affinity"]["podAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"][0];
        assert_eq!(affinity["topologyKey"], "kubernetes.io/hostname");
        assert_eq!(
            affinity["labelSelector"]["matchLabels"]["app.kubernetes.io/component"],
            "server"
        );
    }

    #[test]
    fn the_network_is_a_label_the_charts_policy_selects() {
        let sealed = chart().pod(&pod(Path::new("/work/u"), false)).unwrap();
        let open = chart().pod(&pod(Path::new("/work/u"), true)).unwrap();
        assert_eq!(sealed["metadata"]["labels"]["luu.dev/network"], "none");
        assert_eq!(open["metadata"]["labels"]["luu.dev/network"], "open");
        assert_eq!(
            sealed["metadata"]["labels"]["app.kubernetes.io/instance"],
            "luu"
        );
        assert_eq!(
            sealed["metadata"]["labels"]["app.kubernetes.io/component"],
            "worker"
        );
    }

    #[test]
    fn without_affinity_or_an_owner_the_pod_says_neither() {
        let bare = kube(&[("LUU_KUBE_CLAIM", "work")]).unwrap();
        let pod = bare.pod(&pod(Path::new("/work"), false)).unwrap();
        assert!(pod["spec"].get("affinity").is_none());
        assert!(pod["metadata"].get("ownerReferences").is_none());
        // The claim's root itself: the whole claim, with no subPath.
        assert!(
            pod["spec"]["containers"][0]["volumeMounts"][0]
                .get("subPath")
                .is_none()
        );
    }

    #[test]
    fn the_line_is_kubectl_run_attached_and_removed() {
        let argv = chart()
            .run_argv(&pod(Path::new("/work/u/p"), false))
            .unwrap();
        assert_eq!(
            &argv[..5],
            ["kubectl", "run", "--namespace", "luu", "luu-worker-1-0"]
        );
        for flag in ["--rm", "-i", "--restart=Never", "--override-type=merge"] {
            assert!(argv.iter().any(|word| word == flag), "{flag} in {argv:?}");
        }
        let overrides = argv
            .iter()
            .find_map(|word| word.strip_prefix("--overrides="))
            .expect("the pod");
        let pod: Value = serde_json::from_str(overrides).expect("JSON");
        assert_eq!(pod["spec"]["containers"][0]["name"], "luu-worker-1-0");
    }

    #[test]
    fn pod_names_do_not_repeat_across_starts() {
        let (a, b) = (pod_name(), pod_name());
        assert_ne!(a, b);
        assert!(a.starts_with("luu-worker-") && a.len() <= 63, "{a}");
        assert!(
            a.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        );
    }
}
