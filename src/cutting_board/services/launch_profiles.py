from __future__ import annotations

import json
import os
import tempfile
import threading
from collections.abc import Iterable
from pathlib import Path

from cutting_board.constants import APP_SLUG
from cutting_board.launch_models import LaunchProfile


class LaunchProfileStoreError(RuntimeError):
    pass


class LaunchProfileStore:
    """Persist launch profiles independently from transient UI settings."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path or _default_profiles_path()
        self._lock = threading.RLock()

    def load(self) -> tuple[LaunchProfile, ...]:
        with self._lock:
            try:
                raw = json.loads(self.path.read_text(encoding="utf-8"))
            except FileNotFoundError:
                return ()
            except (PermissionError, OSError, UnicodeError, json.JSONDecodeError) as exc:
                raise LaunchProfileStoreError("Unable to read the launch profile file.") from exc

            if not isinstance(raw, dict) or raw.get("version") != 1:
                raise LaunchProfileStoreError("Unsupported launch profile file format.")
            entries = raw.get("profiles")
            if not isinstance(entries, list):
                raise LaunchProfileStoreError("Invalid launch profile list format.")
            try:
                profiles = tuple(
                    LaunchProfile.from_dict(item)
                    if isinstance(item, dict)
                    else _invalid_profile()
                    for item in entries
                )
                _validate_unique_profiles(profiles)
            except (TypeError, ValueError) as exc:
                raise LaunchProfileStoreError(f"Invalid launch profile configuration: {exc}") from exc
            return profiles

    def save(self, profiles: Iterable[LaunchProfile]) -> tuple[LaunchProfile, ...]:
        checked = tuple(profiles)
        _validate_unique_profiles(checked)
        payload = json.dumps(
            {"version": 1, "profiles": [profile.to_dict() for profile in checked]},
            ensure_ascii=False,
            indent=2,
        ) + "\n"

        with self._lock:
            temporary_path: Path | None = None
            try:
                self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                with tempfile.NamedTemporaryFile(
                    "w",
                    encoding="utf-8",
                    dir=self.path.parent,
                    prefix="launch-profiles-",
                    suffix=".tmp",
                    delete=False,
                ) as handle:
                    temporary_path = Path(handle.name)
                    os.chmod(temporary_path, 0o600)
                    handle.write(payload)
                    handle.flush()
                    os.fsync(handle.fileno())
                os.replace(temporary_path, self.path)
                os.chmod(self.path, 0o600)
            except OSError as exc:
                if temporary_path is not None:
                    try:
                        temporary_path.unlink(missing_ok=True)
                    except OSError:
                        pass
                raise LaunchProfileStoreError("Unable to save launch profiles.") from exc
        return checked

    def upsert(self, profile: LaunchProfile) -> tuple[LaunchProfile, ...]:
        profiles = list(self.load())
        for index, existing in enumerate(profiles):
            if existing.id == profile.id:
                profiles[index] = profile
                break
        else:
            profiles.append(profile)
        return self.save(profiles)

    def delete(self, profile_id: str) -> tuple[LaunchProfile, ...]:
        profiles = tuple(profile for profile in self.load() if profile.id != profile_id)
        return self.save(profiles)


def _default_profiles_path() -> Path:
    config_home = os.environ.get("XDG_CONFIG_HOME")
    base = Path(config_home).expanduser() if config_home else Path.home() / ".config"
    return base / APP_SLUG / "launch_profiles.json"


def _validate_unique_profiles(profiles: tuple[LaunchProfile, ...]) -> None:
    ids: set[str] = set()
    for profile in profiles:
        if not isinstance(profile, LaunchProfile):
            raise TypeError("Invalid launch profile format.")
        if profile.id in ids:
            raise ValueError(f"Duplicate launch profile ID: {profile.id}")
        ids.add(profile.id)


def _invalid_profile() -> LaunchProfile:
    raise TypeError("Invalid launch profile format.")
