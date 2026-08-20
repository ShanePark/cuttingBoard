from __future__ import annotations

import re
from pathlib import Path

from cutting_board.models import ServiceCategory


_GENERIC_PROCESS_NAMES = {
    "node",
    "nodejs",
    "java",
    "python",
    "python3",
    "ruby",
    "go",
    "cargo",
    "dotnet",
    "php",
    "sh",
    "bash",
    "zsh",
}

# Argument values that name a dependency rather than the running program.
# A Spring Boot service is not PostgreSQL just because its classpath ships
# the JDBC driver, so these values never contribute an identity.
_DEPENDENCY_VALUE_FLAGS = {"-cp", "-classpath", "--class-path", "--module-path", "-p"}

_VERSION_SUFFIX = re.compile(r"-[0-9]+(?:\.[0-9A-Za-z_-]+)*$")


class Rule:
    __slots__ = ("needles", "label", "category", "tech")

    def __init__(
        self,
        needles: tuple[str, ...],
        label: str,
        category: ServiceCategory,
        tech: str,
    ) -> None:
        self.needles = needles
        self.label = label
        self.category = category
        self.tech = tech


# Matched against the process name and executable basename only. These are
# real daemons, so their identity is the program that is running.
_DAEMON_RULES: tuple[Rule, ...] = (
    Rule(("postgres", "postmaster"), "PostgreSQL", ServiceCategory.DATABASE, "postgresql"),
    Rule(("mariadbd",), "MariaDB", ServiceCategory.DATABASE, "mariadb"),
    Rule(("mysqld", "mysql"), "MySQL", ServiceCategory.DATABASE, "mysql"),
    Rule(("mongod", "mongodb"), "MongoDB", ServiceCategory.DATABASE, "mongodb"),
    Rule(("redis-server", "redis"), "Redis", ServiceCategory.CACHE, "redis"),
    Rule(("memcached",), "Memcached", ServiceCategory.CACHE, "service"),
    Rule(("docker-proxy",), "Docker port proxy", ServiceCategory.PROXY, "docker"),
    Rule(("dockerd", "containerd", "podman"), "Container runtime", ServiceCategory.PROXY, "docker"),
    Rule(("nginx",), "Nginx", ServiceCategory.PROXY, "nginx"),
    Rule(("caddy",), "Caddy", ServiceCategory.PROXY, "caddy"),
    Rule(("traefik",), "Traefik", ServiceCategory.PROXY, "traefik"),
    Rule(("ollama",), "Ollama", ServiceCategory.RUNTIME, "ollama"),
    Rule(("adb",), "Android Debug Bridge", ServiceCategory.RUNTIME, "android"),
    Rule(("ssh", "sshd"), "SSH tunnel", ServiceCategory.PROXY, "ssh"),
)

# Matched against argv, ignoring dependency values such as the classpath.
_ARGV_RULES: tuple[Rule, ...] = (
    Rule(("vite",), "Vite", ServiceCategory.WEB, "vite"),
    Rule(("next", "next-server"), "Next.js", ServiceCategory.WEB, "nextjs"),
    Rule(("nuxt",), "Nuxt", ServiceCategory.WEB, "nuxt"),
    Rule(("astro",), "Astro", ServiceCategory.WEB, "astro"),
    Rule(("remix",), "Remix", ServiceCategory.WEB, "remix"),
    Rule(("webpack-dev-server", "webpack"), "Webpack Dev Server", ServiceCategory.WEB, "webpack"),
    Rule(("react-scripts",), "React Dev Server", ServiceCategory.WEB, "react"),
    Rule(("storybook", "start-storybook"), "Storybook", ServiceCategory.WEB, "storybook"),
    Rule(("ng",), "Angular Dev Server", ServiceCategory.WEB, "angular"),
    Rule(("svelte-kit", "sveltekit"), "SvelteKit", ServiceCategory.WEB, "svelte"),
    Rule(("uvicorn",), "Uvicorn", ServiceCategory.API, "fastapi"),
    Rule(("gunicorn",), "Gunicorn", ServiceCategory.API, "python"),
    Rule(("hypercorn", "daphne"), "ASGI server", ServiceCategory.API, "python"),
    Rule(("flask",), "Flask", ServiceCategory.API, "flask"),
    Rule(("manage.py",), "Django", ServiceCategory.API, "django"),
    Rule(("jupyter-lab", "jupyter-notebook", "jupyter"), "Jupyter", ServiceCategory.RUNTIME, "jupyter"),
    Rule(("rails",), "Rails", ServiceCategory.API, "rails"),
    Rule(("artisan",), "Laravel", ServiceCategory.API, "laravel"),
    Rule(("cargo",), "Rust service", ServiceCategory.RUNTIME, "rust"),
    Rule(("deno",), "Deno", ServiceCategory.RUNTIME, "deno"),
    Rule(("bun",), "Bun", ServiceCategory.RUNTIME, "bun"),
)

# Matched against dependency values as well. Restricted to launcher and
# framework artefacts, never to drivers or client libraries.
_FRAMEWORK_RULES: tuple[Rule, ...] = (
    Rule(("spring-boot", "bootrun", "springframework"), "Spring Boot", ServiceCategory.API, "spring"),
    Rule(("quarkus",), "Quarkus", ServiceCategory.API, "java"),
    Rule(("micronaut",), "Micronaut", ServiceCategory.API, "java"),
    Rule(("gradle-launcher", "gradledaemon", "gradle"), "Gradle Daemon", ServiceCategory.RUNTIME, "gradle"),
    Rule(("catalina", "tomcat"), "Tomcat", ServiceCategory.API, "tomcat"),
    Rule(("elasticsearch",), "Elasticsearch", ServiceCategory.DATABASE, "elasticsearch"),
    Rule(("solr",), "Solr", ServiceCategory.DATABASE, "solr"),
    Rule(("kafka",), "Kafka", ServiceCategory.RUNTIME, "kafka"),
)

# Runtimes, used only when nothing more specific matched.
_RUNTIME_TECH = {
    "node": "node",
    "nodejs": "node",
    "deno": "deno",
    "bun": "bun",
    "java": "java",
    "python": "python",
    "python3": "python",
    "ruby": "ruby",
    "php": "php",
    "dotnet": "dotnet",
    "go": "go",
}


class Classification:
    """What a listening process appears to be.

    ``specific`` records whether a rule actually recognised the program. A
    false value means the name is only the runtime or the process name, which
    is not enough to call the listener development work.
    """

    __slots__ = ("name", "category", "tech", "specific")

    def __init__(
        self,
        name: str,
        category: ServiceCategory,
        tech: str,
        specific: bool = True,
    ) -> None:
        self.name = name
        self.category = category
        self.tech = tech
        self.specific = specific


def classify_service(
    process_name: str,
    command: tuple[str, ...],
    executable: str | None,
    package_name: str | None,
    project_name: str | None,
) -> Classification:
    identity = _identity_terms(process_name, executable)
    argv_terms = _argv_terms(command, include_dependencies=False)
    all_terms = _argv_terms(command, include_dependencies=True)

    for rule in _DAEMON_RULES:
        if _matches(rule.needles, identity):
            return Classification(
                _with_package(rule.label, package_name, project_name), rule.category, rule.tech
            )

    for rule in _ARGV_RULES:
        if _matches(rule.needles, argv_terms):
            return Classification(
                _with_package(rule.label, package_name, project_name), rule.category, rule.tech
            )

    for rule in _FRAMEWORK_RULES:
        if _matches(rule.needles, all_terms):
            return Classification(
                _with_package(rule.label, package_name, project_name), rule.category, rule.tech
            )

    jar_name = _extract_jar_name(command)
    if jar_name:
        return Classification(
            _with_package(jar_name, package_name, project_name), ServiceCategory.API, "java"
        )

    runtime_tech = _RUNTIME_TECH.get(process_name.casefold(), "service")
    if package_name and package_name != project_name:
        return Classification(package_name, ServiceCategory.OTHER, runtime_tech, specific=False)
    if process_name and process_name.casefold() not in _GENERIC_PROCESS_NAMES:
        return Classification(process_name, ServiceCategory.OTHER, runtime_tech, specific=False)
    if package_name:
        return Classification(package_name, ServiceCategory.OTHER, runtime_tech, specific=False)
    if process_name:
        return Classification(process_name, ServiceCategory.OTHER, runtime_tech, specific=False)
    return Classification("Unknown service", ServiceCategory.OTHER, "service", specific=False)


def classify_image(image: str) -> Classification:
    """What a container image appears to be running.

    Images name their contents far more honestly than a command line does —
    ``postgres:16-alpine`` is unambiguous — so the same rule tables can be
    reused, and every tier is fair game rather than only the daemon tier.
    """
    terms = _image_terms(image)
    for rule in (*_DAEMON_RULES, *_ARGV_RULES, *_FRAMEWORK_RULES):
        if _matches(rule.needles, terms):
            return Classification(rule.label, rule.category, rule.tech)
    return Classification(
        terms[0] if terms else image,
        ServiceCategory.OTHER,
        "docker",
        specific=False,
    )


def _image_terms(image: str) -> tuple[str, ...]:
    """The searchable parts of an image reference.

    ``ghcr.io/acme/billing-api:1.4@sha256:…`` is reduced to the repository
    path and its last segment; the digest, the tag and the registry host carry
    no product identity and only invite false matches.
    """
    reference = image.strip().casefold()
    reference = reference.split("@", 1)[0]
    path, _, tail = reference.rpartition("/")
    # A colon in the last segment is the tag; a colon in the host is a port.
    name = tail.split(":", 1)[0]
    repository = f"{path}/{name}" if path else name
    terms = [name, repository]
    return tuple(dict.fromkeys(term for term in terms if term))


def _identity_terms(process_name: str, executable: str | None) -> tuple[str, ...]:
    terms = [process_name.casefold()]
    if executable:
        terms.append(Path(executable).name.casefold())
    return tuple(term for term in terms if term)


def _argv_terms(command: tuple[str, ...], *, include_dependencies: bool) -> tuple[str, ...]:
    terms: list[str] = []
    skip_next = False
    for token in command:
        if skip_next:
            skip_next = False
            if not include_dependencies:
                continue
        elif token.casefold() in _DEPENDENCY_VALUE_FLAGS:
            skip_next = True

        lowered = token.casefold()
        # Classpaths and PATH-like values pack many entries into one argument.
        for part in lowered.split(":") if ":" in lowered and "/" in lowered else (lowered,):
            name = part.rsplit("/", 1)[-1]
            if name:
                terms.append(_VERSION_SUFFIX.sub("", name.removesuffix(".jar")))
    return tuple(terms)


def _matches(needles: tuple[str, ...], terms: tuple[str, ...]) -> bool:
    """Match a needle only at word boundaries within a term.

    Substring matching would read ``postgresql-42.7.4.jar`` on a classpath as
    PostgreSQL and a directory named ``invite-app`` as Vite.
    """
    for needle in needles:
        pattern = re.compile(rf"(?<![a-z0-9]){re.escape(needle)}(?![a-z0-9])")
        for term in terms:
            if pattern.search(term):
                return True
    return False


def _with_package(label: str, package_name: str | None, project_name: str | None) -> str:
    if package_name and package_name != project_name and package_name.casefold() not in label.casefold():
        return f"{package_name} · {label}"
    return label


def _extract_jar_name(command: tuple[str, ...]) -> str | None:
    for index, token in enumerate(command[:-1]):
        if token == "-jar":
            candidate = Path(command[index + 1]).name
            candidate = _VERSION_SUFFIX.sub("", candidate.removesuffix(".jar"))
            return candidate or "Java service"
    return None
