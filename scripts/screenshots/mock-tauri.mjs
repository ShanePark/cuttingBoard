// Browser-side mock of the Tauri v2 IPC bridge that mirrors the backend demo mode
// (src-tauri/src/scanner/demo.rs, src-tauri/src/docker/demo.rs, demo_profiles). Keep it in step
// with those fixtures and with the commands in src/api.ts; unmocked commands reject loudly.
export function installTauriMock({ themeMode }) {
  const now = Math.floor(Date.now() / 1000);
  const DEMO_REJECT = "Actions are disabled in demonstration mode.";

  const service = (id, name, tech, category, port, path, age, originKind, origin) => ({
    id,
    display_name: name,
    tech,
    category,
    relevance: "dev",
    endpoints: [{ family: "IPv4", address: "127.0.0.1", port, scope: "loopback", protocol: "TCP" }],
    process: {
      pid: 20000 + port,
      parent_pid: 1000,
      name: tech,
      executable: `/usr/local/bin/${tech}`,
      working_directory: path,
      command: `${tech} run --port ${port}`,
      launch_command: `${tech} run --port ${port}`,
      create_time: now - age,
      uptime_seconds: age,
      cpu_percent: 1.4,
      memory_bytes: 146800640,
      uid: 1000
    },
    project: {
      id: `project-${id}`,
      name: path.split("/").pop() || name,
      root_path: path,
      workspace_root_path: path,
      workspace_name: path.split("/").pop() || name,
      detection_source: "package.json"
    },
    status: "healthy",
    warnings: [],
    origin_kind: originKind,
    origin_label: origin,
    can_terminate: false,
    browser_url: `http://localhost:${port}`,
    active_profiles: []
  });

  const services = [
    service("demo-api", "cutting-board-api · Spring Boot", "spring", "api", 8080, "/Users/shane/Developer/cutting-board-api", 94, "ide", "IntelliJ IDEA"),
    service("demo-web", "cutting-board-web · Vite", "vite", "web", 5173, "/Users/shane/Developer/cutting-board-web", 3725, "agent", "Agent"),
    service("demo-db", "local-postgres", "postgresql", "database", 5432, "/Users/shane/Developer/local-stack", 86400, "terminal", "iTerm2"),
    service("demo-cache", "local-redis", "redis", "cache", 6379, "/Users/shane/Developer/local-stack", 710, "terminal", "iTerm2")
  ];

  const mapping = (port) => ({ host_ip: "0.0.0.0", host_port: port, container_port: port, protocol: "tcp" });
  const containerListing = {
    available: true,
    message: null,
    containers: [
      { id: "4d2a92d5c814", name: "local-postgres", image: "postgres:16", state: "running", status: "Up 2 hours", ports: [5432], port_mappings: [mapping(5432)], compose_project: "local-stack", compose_service: "database", compose_working_dir: "/Users/shane/Developer/local-stack" },
      { id: "e9396d773a34", name: "mailpit", image: "axllent/mailpit:latest", state: "running", status: "Up 2 hours", ports: [1025, 8025], port_mappings: [mapping(1025), mapping(8025)], compose_project: "local-stack", compose_service: "mail", compose_working_dir: "/Users/shane/Developer/local-stack" },
      { id: "91fd0fb2d381", name: "old-redis", image: "redis:7", state: "exited", status: "Exited (0) 3 days ago", ports: [], port_mappings: [], compose_project: null, compose_service: null, compose_working_dir: null }
    ]
  };
  const containerLogs = {
    "4d2a92d5c814": "2026-01-01T00:00:00.000000000Z database system is ready to accept connections\n",
    "e9396d773a34": "2026-01-01T00:00:00.000000000Z [INFO] mailpit listening on :8025\n",
    "91fd0fb2d381": "2025-12-29T00:00:00.000000000Z Ready to accept connections\n"
  };

  const profiles = [{
    id: "demo-profile",
    name: "Cutting Board Demo",
    project_root: "/Users/shane/Developer/cutting-board-demo",
    tasks: [
      { name: "API", cwd: "backend", command: "./gradlew bootRun", expected_port: 8080, container: null, prepare: null },
      { name: "Frontend", cwd: "frontend", command: "npm run dev", expected_port: 5173, container: null, prepare: null }
    ]
  }];
  const taskSnapshots = () => profiles.flatMap((profile) => profile.tasks.map((task) => ({
    profile_id: profile.id,
    task_name: task.name,
    state: "stopped",
    main_pid: null,
    started_at: null,
    exit_code: null,
    message: null,
    log_tail: "",
    external_pid: null,
    external_working_directory: null,
    external_log_path: null
  })));

  let settings = {
    theme_mode: themeMode,
    scan_interval_ms: 2000,
    window_width: 1080,
    window_height: 720,
    window_x: null,
    window_y: null,
    window_geometry_logical: false
  };

  const clone = (value) => JSON.parse(JSON.stringify(value));
  const handlers = {
    app_info: () => ({
      version: "0.6.0",
      demo: true,
      settings_path: "/home/demo/.config/cutting-board/settings.json",
      profiles_path: "/home/demo/.config/cutting-board/profiles.json",
      update_supported: false
    }),
    system_metrics: () => ({ cpu_percent: 23.4, memory_percent: 61.8 }),
    scan_workspace: () => ({
      services: clone(services),
      scanned_at: Math.floor(Date.now() / 1000),
      scan_duration_ms: 18,
      endpoint_count: services.length,
      errors: []
    }),
    list_containers: () => clone(containerListing),
    container_logs: (args) => ({ logs: containerLogs[args?.request?.container_id] ?? "" }),
    service_logs: () => ({
      logs: "",
      source_path: null,
      available: false,
      message: "Service logs are unavailable in demonstration mode."
    }),
    load_settings: () => clone(settings),
    save_settings: (args) => {
      settings = { ...settings, ...(args?.settings ?? {}) };
      return clone(settings);
    },
    load_profiles: () => clone(profiles),
    task_snapshots: () => taskSnapshots(),
    task_log_tail: () => "",
    compose_down_log: () => "",
    check_for_update: () => ({ available: false, current_commit: "demo", latest_commit: "demo" }),
    "plugin:event|listen": () => Math.floor(Math.random() * 1e9),
    "plugin:event|unlisten": () => null,
    "plugin:event|emit": () => null,
    "plugin:opener|open_url": () => null,
    "plugin:opener|open_path": () => null,
    "plugin:dialog|open": () => null
  };
  const rejected = new Set([
    "start_container", "stop_container", "compose_down", "terminate_service", "restart_service",
    "save_profile", "delete_profile", "start_task", "stop_task", "restart_task", "stop_profile",
    "update_and_restart", "shutdown"
  ]);

  const callbacks = new Map();
  let nextCallbackId = 1;
  window.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" }
    },
    plugins: { path: { sep: "/", delimiter: ":" } },
    callbacks,
    transformCallback(callback, once = false) {
      const id = nextCallbackId++;
      callbacks.set(id, (data) => {
        if (once) callbacks.delete(id);
        return callback && callback(data);
      });
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    runCallback(id, data) {
      callbacks.get(id)?.(data);
    },
    convertFileSrc(filePath, protocol = "asset") {
      return `${protocol}://localhost/${encodeURIComponent(filePath)}`;
    },
    async invoke(cmd, args = {}) {
      window.__TAURI_MOCK_CALLS__ = window.__TAURI_MOCK_CALLS__ || [];
      window.__TAURI_MOCK_CALLS__.push(cmd);
      if (rejected.has(cmd)) throw DEMO_REJECT;
      const handler = handlers[cmd];
      if (!handler) throw `Command ${cmd} is not mocked by the screenshot harness.`;
      return handler(args);
    }
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener() {}
  };
}
