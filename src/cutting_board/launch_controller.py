from __future__ import annotations

import queue
import threading

from cutting_board.launch_models import LaunchEvent, LaunchProfile, ManagedTaskSnapshot
from cutting_board.services.launch_profiles import LaunchProfileStore
from cutting_board.services.managed_processes import ManagedProcessError, ManagedProcessRunner


class LaunchController:
    """Coordinate persisted launch profiles with app-owned process groups."""

    def __init__(
        self,
        store: LaunchProfileStore | None = None,
        runner: ManagedProcessRunner | None = None,
    ) -> None:
        self._store = store or LaunchProfileStore()
        self._runner = runner or ManagedProcessRunner()
        self._lock = threading.RLock()
        self._profiles = self._store.load()
        self._closed = False

    @property
    def events(self) -> queue.Queue[LaunchEvent]:
        return self._runner.events

    @property
    def profiles(self) -> tuple[LaunchProfile, ...]:
        with self._lock:
            return self._profiles

    def profile(self, profile_id: str) -> LaunchProfile:
        with self._lock:
            return self._profile_locked(profile_id)

    def save_profile(self, profile: LaunchProfile) -> tuple[LaunchProfile, ...]:
        with self._lock:
            self._ensure_open_locked()
            if self._runner.is_profile_active(profile.id):
                raise ManagedProcessError("실행 중인 프로필은 수정할 수 없습니다.")
            profiles = list(self._profiles)
            for index, existing in enumerate(profiles):
                if existing.id == profile.id:
                    profiles[index] = profile
                    break
            else:
                profiles.append(profile)
            saved = self._store.save(profiles)
            self._profiles = saved
            return saved

    def delete_profile(self, profile_id: str) -> bool:
        with self._lock:
            self._ensure_open_locked()
            if self._runner.is_profile_active(profile_id):
                raise ManagedProcessError("실행 중인 프로필은 삭제할 수 없습니다.")
            if not any(profile.id == profile_id for profile in self._profiles):
                return False
            profiles = tuple(profile for profile in self._profiles if profile.id != profile_id)
            saved = self._store.save(profiles)
            self._profiles = saved
            return True

    def start_task(self, profile_id: str, task_name: str) -> ManagedTaskSnapshot:
        with self._lock:
            self._ensure_open_locked()
            profile = self._profile_locked(profile_id)
        return self._runner.start_task(profile, task_name)

    def start_profile(self, profile_id: str) -> tuple[ManagedTaskSnapshot, ...]:
        with self._lock:
            self._ensure_open_locked()
            profile = self._profile_locked(profile_id)
        return self._runner.start_profile(profile)

    def stop_task(self, profile_id: str, task_name: str) -> ManagedTaskSnapshot:
        return self._runner.stop_task(profile_id, task_name)

    def stop_profile(self, profile_id: str) -> tuple[ManagedTaskSnapshot, ...]:
        return self._runner.stop_profile(profile_id)

    def restart_task(self, profile_id: str, task_name: str) -> ManagedTaskSnapshot:
        self.stop_task(profile_id, task_name)
        return self.start_task(profile_id, task_name)

    def snapshot(self, profile_id: str, task_name: str) -> ManagedTaskSnapshot:
        return self._runner.snapshot(profile_id, task_name)

    def snapshots(self, profile_id: str | None = None) -> tuple[ManagedTaskSnapshot, ...]:
        return self._runner.snapshots(profile_id)

    def close(self) -> None:
        with self._lock:
            if self._closed:
                return
            self._closed = True
        self._runner.close()

    def _profile_locked(self, profile_id: str) -> LaunchProfile:
        for profile in self._profiles:
            if profile.id == profile_id:
                return profile
        raise KeyError(f"등록되지 않은 실행 프로필입니다: {profile_id}")

    def _ensure_open_locked(self) -> None:
        if self._closed:
            raise ManagedProcessError("실행 관리자가 이미 종료되었습니다.")
