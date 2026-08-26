use crate::models::ProjectInfo;
use std::path::Path;

#[derive(Debug)]
pub(crate) struct Classification {
    pub(crate) tech: String,
    pub(crate) category: String,
    pub(crate) relevance: String,
}

pub(crate) fn classify(
    process_name: &str,
    executable: Option<&str>,
    args: &[String],
    ports: &[u16],
    project: Option<&ProjectInfo>,
) -> Classification {
    if is_macos_application_executable(executable) {
        return Classification {
            tech: "generic".into(),
            category: "other".into(),
            relevance: "noise".into(),
        };
    }

    let identity = process_identity(process_name, executable, args);
    let command = args.join(" ").to_lowercase();
    let searchable = format!("{} {}", identity.join(" "), command);

    if contains_any(
        &searchable,
        &[
            "docker-proxy",
            "com.docker",
            "vpnkit",
            "containerd-shim",
            "podman machine",
        ],
    ) {
        return Classification {
            tech: "docker".into(),
            category: "runtime".into(),
            relevance: "container".into(),
        };
    }
    if is_desktop_or_system_noise(&identity, &command) || is_build_daemon(&identity, args) {
        return Classification {
            tech: "generic".into(),
            category: "other".into(),
            relevance: "noise".into(),
        };
    }

    if contains_any(
        &searchable,
        &[
            "launchd",
            "systemd",
            "windowserver",
            "controlcenter",
            "coreservices",
            "sharingd",
            "rapportd",
            "identityservicesd",
            "airportd",
            "bluetoothd",
            "cupsd",
            "avahi-daemon",
            "gnome-shell",
            "kdeconnectd",
            "notificationcenter",
            "distnoted",
            "trustd",
            "opendirectoryd",
        ],
    ) {
        return Classification {
            tech: "generic".into(),
            category: "other".into(),
            relevance: "noise".into(),
        };
    }

    let daemon_mappings: &[(&[&str], &str, &str)] = &[
        (&["postgres", "postmaster"], "postgresql", "database"),
        (&["mariadbd"], "mariadb", "database"),
        (&["mysqld", "mysql"], "mysql", "database"),
        (&["mongod", "mongodb"], "mongodb", "database"),
        (&["redis-server", "redis"], "redis", "cache"),
        (&["memcached"], "memcached", "cache"),
        (&["elasticsearch"], "elasticsearch", "database"),
        (&["rabbitmq"], "rabbitmq", "cache"),
        (&["nginx"], "nginx", "proxy"),
        (&["caddy"], "caddy", "proxy"),
        (&["traefik"], "traefik", "proxy"),
        (&["ollama"], "ollama", "runtime"),
        (&["ssh", "sshd"], "ssh", "proxy"),
    ];
    for (needles, tech, category) in daemon_mappings {
        if identity
            .iter()
            .any(|term| needles.iter().any(|needle| term == needle))
        {
            return Classification {
                tech: (*tech).into(),
                category: (*category).into(),
                relevance: "dev".into(),
            };
        }
    }

    if identity
        .iter()
        .any(|term| matches!(term.as_str(), "java" | "java.exe"))
        && project.is_some_and(super::project::project_has_spring_evidence)
    {
        return Classification {
            tech: "spring".into(),
            category: "api".into(),
            relevance: "dev".into(),
        };
    }

    let mappings: &[(&[&str], &str, &str)] = &[
        (&["spring boot", "spring-boot", "bootrun"], "spring", "api"),
        (&["next dev", "next-server", "nextjs"], "nextjs", "web"),
        (&["vite", "vite.js"], "vite", "web"),
        (&["nuxt"], "nuxt", "web"),
        (&["astro"], "astro", "web"),
        (&["remix"], "remix", "web"),
        (&["webpack-dev-server", "react-scripts"], "nodejs", "web"),
        (&["storybook", "start-storybook"], "storybook", "web"),
        (&["svelte-kit", "sveltekit"], "svelte", "web"),
        (&["angular", "ng serve"], "angular", "web"),
        (&["django", "manage.py runserver"], "django", "web"),
        (&["fastapi", "uvicorn"], "fastapi", "api"),
        (&["gunicorn", "hypercorn", "daphne"], "python", "api"),
        (&["flask"], "flask", "web"),
        (&["rails", "puma"], "rails", "web"),
        (&["artisan"], "laravel", "api"),
        (&["jupyter-lab", "jupyter-notebook"], "jupyter", "runtime"),
        (&["quarkus"], "java", "api"),
        (&["micronaut"], "java", "api"),
        (&["catalina", "tomcat"], "tomcat", "api"),
        (&["solr"], "solr", "database"),
        (&["kafka"], "kafka", "runtime"),
        (&["deno"], "deno", "runtime"),
        (&["bun"], "bun", "runtime"),
        (
            &["cargo run", "target/debug", "target/release"],
            "rust",
            "runtime",
        ),
        (&["go run", "/go/bin/"], "go", "runtime"),
    ];
    for (needles, tech, category) in mappings {
        if contains_any_term(&searchable, needles) {
            return Classification {
                tech: (*tech).into(),
                category: (*category).into(),
                relevance: "dev".into(),
            };
        }
    }

    let common_dev_port = ports.iter().any(|port| {
        matches!(
            port,
            3000 | 3001
                | 4000
                | 4173
                | 4200
                | 4321
                | 5000
                | 5173
                | 5432
                | 6379
                | 8000
                | 8001
                | 8080
                | 8081
                | 8443
                | 8888
                | 9000
                | 9090
                | 9200
                | 27017
        )
    });
    let tech = runtime_tech(&identity).unwrap_or("generic");
    let system_executable = executable.is_some_and(|value| {
        [
            "/usr/lib",
            "/usr/libexec",
            "/usr/share",
            "/usr/sbin",
            "/opt",
            "/snap",
            "/var/lib/flatpak",
            "/var/lib/snapd",
        ]
        .iter()
        .any(|prefix| value.starts_with(prefix))
    });
    let is_dev = !system_executable && (project.is_some() || common_dev_port);
    Classification {
        tech: tech.into(),
        category: if common_dev_port {
            "web".into()
        } else {
            "other".into()
        },
        relevance: if is_dev { "dev".into() } else { "noise".into() },
    }
}

fn is_macos_application_executable(executable: Option<&str>) -> bool {
    executable.is_some_and(|value| Path::new(value).starts_with("/Applications"))
}

fn process_identity(process_name: &str, executable: Option<&str>, args: &[String]) -> Vec<String> {
    let mut identity = vec![process_name.to_lowercase()];
    if let Some(name) = executable.and_then(|value| Path::new(value).file_name()) {
        identity.push(name.to_string_lossy().to_lowercase());
    }
    for argument in args
        .iter()
        .skip(1)
        .filter(|value| !value.starts_with('-'))
        .take(2)
    {
        if let Some(name) = Path::new(argument).file_name() {
            identity.push(name.to_string_lossy().to_lowercase());
        }
    }
    identity
}

fn runtime_tech(identity: &[String]) -> Option<&'static str> {
    for name in identity {
        let tech = match name.as_str() {
            "node" | "nodejs" => "nodejs",
            "python" | "python3" => "python",
            "java" | "java.exe" => "java",
            "dotnet" => "dotnet",
            "ruby" => "ruby",
            "php" => "php",
            "go" => "go",
            "cargo" => "rust",
            "deno" => "deno",
            "bun" => "bun",
            _ => continue,
        };
        return Some(tech);
    }
    None
}

fn is_desktop_or_system_noise(identity: &[String], command: &str) -> bool {
    const DESKTOP_APPS: &[&str] = &[
        "ulauncher",
        "kdeconnectd",
        "kdeconnect-indicator",
        "dropbox",
        "insync",
        "nextcloud",
        "megasync",
        "syncthing",
        "spotify",
        "steam",
        "steamwebhelper",
        "discord",
        "slack",
        "telegram-desktop",
        "signal-desktop",
        "element-desktop",
        "zoom",
        "teams",
        "skype",
        "github-desktop",
        "jetbrains-toolbox",
        "anydesk",
        "teamviewer",
        "rustdesk",
        "barrier",
        "synergy",
        "obs",
        "kdeinit5",
        "plasmashell",
        "gnome-shell",
        "evolution",
        "thunderbird",
        "firefox",
        "chrome",
        "chromium",
        "brave",
        "vivaldi",
        "opera",
        "transmission",
        "qbittorrent",
        "deluge",
        "vlc",
        "kodi",
        "warp",
        "1password",
        "bitwarden",
        "keepassxc",
    ];
    const IDE_MARKERS: &[&str] = &[
        "com.intellij.idea.main",
        "org.jetbrains.jps.cmdline.launcher",
        "kotlincompiledaemon",
        "daemonmavencli",
        "org.mvndaemon.mvnd.daemon.server",
    ];
    identity
        .iter()
        .any(|name| DESKTOP_APPS.contains(&name.as_str()))
        || contains_any(command, IDE_MARKERS)
}

pub(crate) fn is_build_daemon(identity: &[String], args: &[String]) -> bool {
    if identity
        .iter()
        .any(|name| matches!(name.as_str(), "mvnd" | "mvnd.exe"))
    {
        return true;
    }
    const MAIN_CLASSES: &[&str] = &[
        "org.gradle.launcher.daemon.bootstrap.gradledaemon",
        "org.gradle.process.internal.worker.gradleworkermain",
        "org.jetbrains.kotlin.daemon.kotlincompiledaemon",
        "org.apache.maven.cli.daemonmavencli",
        "org.mvndaemon.mvnd.daemon.server",
        "com.facebook.nailgun.ngserver",
        "com.martiansoftware.nailgun.ngserver",
    ];
    args.iter().any(|argument| {
        let lowered = argument.to_lowercase();
        if MAIN_CLASSES.contains(&lowered.as_str()) {
            return true;
        }
        lowered.split([':', ';']).any(|entry| {
            let name = entry.rsplit(['/', '\\']).next().unwrap_or(entry);
            name.ends_with(".jar")
                && (name.starts_with("gradle-daemon-main") || name.starts_with("gradle-worker"))
        })
    })
}

fn contains_any_term(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        text.match_indices(needle).any(|(start, matched)| {
            let before = text[..start].chars().next_back();
            let after = text[start + matched.len()..].chars().next();
            !before.is_some_and(|value| value.is_ascii_alphanumeric())
                && !after.is_some_and(|value| value.is_ascii_alphanumeric())
        })
    })
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}
