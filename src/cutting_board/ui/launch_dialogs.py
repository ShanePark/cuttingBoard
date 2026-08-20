from __future__ import annotations

import tkinter as tk
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from tkinter import filedialog

from cutting_board.ui import theme
from cutting_board.ui.dialogs import ModalBackdrop, configure_detail_dismiss
from cutting_board.ui.launch_widgets import ActionButton


@dataclass(frozen=True, slots=True)
class LaunchTaskDraft:
    name: str
    cwd: str
    command: str
    expected_port: int | None = None
    watch_command: str | None = None


@dataclass(frozen=True, slots=True)
class LaunchProfileDraft:
    name: str
    project_root: str
    tasks: tuple[LaunchTaskDraft, ...] = field(default_factory=tuple)


class LaunchProfileValidationError(ValueError):
    """A profile form error suitable for showing directly in the UI."""


def validate_profile_draft(draft: LaunchProfileDraft) -> LaunchProfileDraft:
    """Validate and normalize the value collected by the profile form."""
    name = draft.name.strip()
    if not name:
        raise LaunchProfileValidationError("구성 이름을 입력해 주세요.")
    root_text = draft.project_root.strip()
    if not root_text:
        raise LaunchProfileValidationError("프로젝트 폴더를 입력해 주세요.")
    root = Path(root_text).expanduser()
    if not root.is_absolute():
        raise LaunchProfileValidationError("프로젝트 폴더는 절대 경로로 입력해 주세요.")
    root = root.resolve(strict=False)
    if not draft.tasks:
        raise LaunchProfileValidationError("실행 작업을 하나 이상 추가해 주세요.")

    names: set[str] = set()
    tasks: list[LaunchTaskDraft] = []
    for index, task in enumerate(draft.tasks, start=1):
        task_name = task.name.strip()
        if not task_name:
            raise LaunchProfileValidationError(f"{index}번째 작업 이름을 입력해 주세요.")
        folded = task_name.casefold()
        if folded in names:
            raise LaunchProfileValidationError(f"작업 이름이 중복되었습니다: {task_name}")
        names.add(folded)

        cwd_text = task.cwd.strip()
        if not cwd_text:
            raise LaunchProfileValidationError(f"'{task_name}' 작업 폴더를 입력해 주세요.")
        configured_cwd = Path(cwd_text).expanduser()
        cwd = (
            configured_cwd.resolve(strict=False)
            if configured_cwd.is_absolute()
            else (root / configured_cwd).resolve(strict=False)
        )
        try:
            cwd.relative_to(root)
        except ValueError as exc:
            raise LaunchProfileValidationError(
                f"'{task_name}' 작업 폴더는 프로젝트 안에 있어야 합니다."
            ) from exc

        command = task.command.strip()
        if not command:
            raise LaunchProfileValidationError(f"'{task_name}' 실행 명령을 입력해 주세요.")
        watch_command = task.watch_command.strip() if task.watch_command else None
        tasks.append(
            LaunchTaskDraft(
                name=task_name,
                cwd=cwd_text,
                command=command,
                expected_port=_validated_port(task.expected_port, task_name),
                watch_command=watch_command,
            )
        )
    return LaunchProfileDraft(name=name, project_root=str(root), tasks=tuple(tasks))


def _validated_port(value: int | None, task_name: str) -> int | None:
    if value is None:
        return None
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 65535:
        raise LaunchProfileValidationError(f"'{task_name}' 예상 포트는 1~65535로 입력해 주세요.")
    return value


class LaunchProfileDialog(tk.Toplevel):
    """Add or edit a profile with a repeatable list of launch tasks."""

    def __init__(
        self,
        parent: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        profile: LaunchProfileDraft | None = None,
        on_save: Callable[[LaunchProfileDraft], None] | None = None,
    ) -> None:
        backdrop = ModalBackdrop(parent)
        super().__init__(backdrop.window, bg=theme.CANVAS)
        self._modal_backdrop = backdrop
        self._fonts = fonts
        self._on_save = on_save
        self.result: LaunchProfileDraft | None = None
        self._task_editors: list[_TaskEditor] = []

        self.title("실행 구성 편집" if profile else "실행 구성 추가")
        self.transient(backdrop.window)
        self.minsize(720, 520)
        self.geometry("780x680")
        self._build(profile)
        self.bind("<Escape>", lambda _event: self.destroy())
        self.protocol("WM_DELETE_WINDOW", self.destroy)
        self.update_idletasks()
        backdrop.activate(self, on_outside=None)

    def _build(self, profile: LaunchProfileDraft | None) -> None:
        header = tk.Frame(self, bg=theme.SURFACE, padx=24, pady=18)
        header.pack(fill="x")
        tk.Label(
            header,
            text=self.title(),
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=(self._fonts["body_bold"][0], 15, "bold"),
            anchor="w",
        ).pack(fill="x")
        tk.Label(
            header,
            text="프로젝트에서 함께 실행할 작업과 자동 빌드 명령을 등록합니다.",
            bg=theme.SURFACE,
            fg=theme.TEXT_MUTED,
            font=self._fonts["small"],
            anchor="w",
        ).pack(fill="x", pady=(4, 0))

        content = tk.Frame(self, bg=theme.CANVAS)
        content.pack(fill="both", expand=True)
        self._canvas = tk.Canvas(content, bg=theme.CANVAS, highlightthickness=0, bd=0)
        scrollbar = tk.Scrollbar(
            content,
            orient="vertical",
            command=self._canvas.yview,
            bg=theme.BORDER,
            troughcolor=theme.CANVAS,
            activebackground=theme.ACCENT_DIM,
            width=9,
        )
        self._form = tk.Frame(self._canvas, bg=theme.CANVAS, padx=24, pady=18)
        self._form_window = self._canvas.create_window((0, 0), window=self._form, anchor="nw")
        self._canvas.configure(yscrollcommand=scrollbar.set)
        self._canvas.pack(side="left", fill="both", expand=True)
        scrollbar.pack(side="right", fill="y")
        self._form.bind(
            "<Configure>",
            lambda _event: self._canvas.configure(scrollregion=self._canvas.bbox("all")),
        )
        self._canvas.bind(
            "<Configure>",
            lambda event: self._canvas.itemconfigure(self._form_window, width=event.width),
        )

        self._name = self._field(self._form, "구성 이름", profile.name if profile else "")
        root_row = tk.Frame(self._form, bg=theme.CANVAS)
        root_row.pack(fill="x", pady=(0, 14))
        root_field = tk.Frame(root_row, bg=theme.CANVAS)
        root_field.pack(side="left", fill="x", expand=True)
        self._project_root = self._field(
            root_field,
            "프로젝트 폴더",
            profile.project_root if profile else "",
            bottom_pad=0,
        )
        ActionButton(
            root_row,
            text="찾아보기",
            fonts=self._fonts,
            command=self._choose_root,
            compact=True,
        ).pack(side="right", padx=(8, 0), pady=(22, 0))

        tasks_head = tk.Frame(self._form, bg=theme.CANVAS)
        tasks_head.pack(fill="x", pady=(8, 8))
        tk.Label(
            tasks_head,
            text="실행 작업",
            bg=theme.CANVAS,
            fg=theme.TEXT,
            font=self._fonts["section"],
        ).pack(side="left")
        ActionButton(
            tasks_head,
            text="작업 추가",
            fonts=self._fonts,
            command=self._add_task,
            foreground=theme.ACCENT,
            compact=True,
        ).pack(side="right")
        self._tasks = tk.Frame(self._form, bg=theme.CANVAS)
        self._tasks.pack(fill="x")
        initial_tasks = profile.tasks if profile else (LaunchTaskDraft("", ".", ""),)
        for task in initial_tasks:
            self._add_task(task)

        self._error = tk.Label(
            self._form,
            text="",
            bg=theme.CANVAS,
            fg=theme.DANGER,
            font=self._fonts["small"],
            anchor="w",
            justify="left",
        )
        self._error.pack(fill="x", pady=(12, 0))

        footer = tk.Frame(self, bg=theme.SURFACE, padx=24, pady=14)
        footer.pack(fill="x", side="bottom")
        ActionButton(
            footer,
            text="저장",
            fonts=self._fonts,
            command=self._save,
            foreground=theme.ON_ACCENT,
            background=theme.ACCENT,
            hover=theme.ACCENT_HOVER,
        ).pack(side="right")
        ActionButton(
            footer,
            text="취소",
            fonts=self._fonts,
            command=self.destroy,
        ).pack(side="right", padx=(0, 8))

        self.after_idle(self._name.focus_set)

    def _field(
        self,
        parent: tk.Misc,
        label: str,
        value: str,
        *,
        bottom_pad: int = 14,
    ) -> tk.Entry:
        tk.Label(
            parent,
            text=label,
            bg=theme.CANVAS,
            fg=theme.TEXT_MUTED,
            font=self._fonts["label"],
            anchor="w",
        ).pack(fill="x", pady=(0, 5))
        entry = _entry(parent, self._fonts, value)
        entry.pack(fill="x", ipady=6, pady=(0, bottom_pad))
        return entry

    def _choose_root(self) -> None:
        selected = filedialog.askdirectory(
            parent=self,
            title="프로젝트 폴더 선택",
            initialdir=self._project_root.get() or str(Path.home()),
        )
        if selected:
            self._project_root.delete(0, "end")
            self._project_root.insert(0, selected)

    def _add_task(self, task: LaunchTaskDraft | None = None) -> None:
        editor = _TaskEditor(
            self._tasks,
            fonts=self._fonts,
            task=task or LaunchTaskDraft("", ".", ""),
            on_remove=self._remove_task,
        )
        self._task_editors.append(editor)
        self._refresh_task_numbers()
        self.after_idle(lambda: self._canvas.yview_moveto(1.0))

    def _remove_task(self, editor: _TaskEditor) -> None:
        if len(self._task_editors) == 1:
            self._error.configure(text="실행 작업은 하나 이상 필요합니다.")
            return
        self._task_editors.remove(editor)
        editor.destroy()
        self._refresh_task_numbers()

    def _refresh_task_numbers(self) -> None:
        for index, editor in enumerate(self._task_editors, start=1):
            editor.set_number(index)

    def _collect(self) -> LaunchProfileDraft:
        return LaunchProfileDraft(
            name=self._name.get(),
            project_root=self._project_root.get(),
            tasks=tuple(editor.draft() for editor in self._task_editors),
        )

    def _save(self) -> None:
        try:
            result = validate_profile_draft(self._collect())
        except LaunchProfileValidationError as exc:
            self._error.configure(text=str(exc))
            return
        self.result = result
        if self._on_save is not None:
            self._on_save(result)
        self.destroy()


class _TaskEditor(tk.Frame):
    def __init__(
        self,
        master: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        task: LaunchTaskDraft,
        on_remove: Callable[[_TaskEditor], None],
    ) -> None:
        super().__init__(
            master,
            bg=theme.SURFACE,
            highlightthickness=1,
            highlightbackground=theme.BORDER,
            padx=14,
            pady=12,
        )
        self.pack(fill="x", pady=(0, 10))
        self._fonts = fonts
        self._on_remove = on_remove

        head = tk.Frame(self, bg=theme.SURFACE)
        head.pack(fill="x", pady=(0, 9))
        self._title = tk.Label(
            head,
            text="",
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=fonts["body_bold"],
        )
        self._title.pack(side="left")
        ActionButton(
            head,
            text="제거",
            fonts=fonts,
            command=lambda: self._on_remove(self),
            foreground=theme.DANGER,
            compact=True,
        ).pack(side="right")

        first = tk.Frame(self, bg=theme.SURFACE)
        first.pack(fill="x")
        name_holder, self._name = self._labelled_entry(first, "작업 이름", task.name, width=22)
        name_holder.pack(side="left", fill="x", expand=True)
        cwd_holder, self._cwd = self._labelled_entry(
            first,
            "작업 폴더 (상대 또는 절대 경로)",
            task.cwd,
        )
        cwd_holder.pack(side="left", fill="x", expand=True, padx=(10, 0))

        self._command = self._wide_entry("실행 명령", task.command)
        second = tk.Frame(self, bg=theme.SURFACE)
        second.pack(fill="x", pady=(9, 0))
        port_text = str(task.expected_port) if task.expected_port is not None else ""
        port_holder, self._port = self._labelled_entry(
            second,
            "예상 포트 (선택)",
            port_text,
            width=10,
        )
        port_holder.pack(side="left")
        watch_holder, self._watch = self._labelled_entry(
            second,
            "자동 빌드 / 감시 명령 (선택)",
            task.watch_command or "",
        )
        watch_holder.pack(side="left", fill="x", expand=True, padx=(10, 0))

    def set_number(self, number: int) -> None:
        self._title.configure(text=f"작업 {number}")

    def draft(self) -> LaunchTaskDraft:
        raw_port = self._port.get().strip()
        if raw_port:
            try:
                port: int | None = int(raw_port)
            except ValueError as exc:
                name = self._name.get().strip() or "이름 없는 작업"
                raise LaunchProfileValidationError(
                    f"'{name}' 예상 포트는 숫자로 입력해 주세요."
                ) from exc
        else:
            port = None
        return LaunchTaskDraft(
            name=self._name.get(),
            cwd=self._cwd.get(),
            command=self._command.get(),
            expected_port=port,
            watch_command=self._watch.get() or None,
        )

    def _wide_entry(self, label: str, value: str) -> tk.Entry:
        holder = tk.Frame(self, bg=theme.SURFACE)
        holder.pack(fill="x", pady=(9, 0))
        field, entry = self._labelled_entry(holder, label, value)
        field.pack(fill="x")
        return entry

    def _labelled_entry(
        self,
        parent: tk.Misc,
        label: str,
        value: str,
        *,
        width: int | None = None,
    ) -> tuple[tk.Frame, tk.Entry]:
        holder = tk.Frame(parent, bg=theme.SURFACE)
        tk.Label(
            holder,
            text=label,
            bg=theme.SURFACE,
            fg=theme.TEXT_MUTED,
            font=self._fonts["small"],
            anchor="w",
        ).pack(fill="x", pady=(0, 4))
        entry = _entry(holder, self._fonts, value, width=width)
        entry.pack(fill="x", ipady=5)
        return holder, entry


class LaunchLogDialog(tk.Toplevel):
    """Read-only in-memory output for one managed task."""

    def __init__(
        self,
        parent: tk.Misc,
        *,
        fonts: dict[str, tuple[str, int, str]],
        profile_name: str,
        task_name: str,
        lines: Sequence[str],
    ) -> None:
        backdrop = ModalBackdrop(parent)
        super().__init__(backdrop.window, bg=theme.CANVAS)
        self._modal_backdrop = backdrop
        self.title(f"{task_name} 로그")
        self.transient(backdrop.window)
        self.geometry("820x520")
        self.minsize(620, 360)

        header = tk.Frame(self, bg=theme.SURFACE, padx=20, pady=14)
        header.pack(fill="x")
        tk.Label(
            header,
            text=task_name,
            bg=theme.SURFACE,
            fg=theme.TEXT,
            font=fonts["section"],
            anchor="w",
        ).pack(fill="x")
        tk.Label(
            header,
            text=profile_name,
            bg=theme.SURFACE,
            fg=theme.TEXT_MUTED,
            font=fonts["small"],
            anchor="w",
        ).pack(fill="x", pady=(3, 0))

        body = tk.Frame(self, bg=theme.CANVAS, padx=16, pady=16)
        body.pack(fill="both", expand=True)
        output = tk.Text(
            body,
            bg=theme.SURFACE,
            fg=theme.TEXT,
            insertbackground=theme.ACCENT,
            selectbackground=theme.ACCENT_DIM,
            font=fonts["mono"],
            wrap="none",
            relief="flat",
            highlightthickness=1,
            highlightbackground=theme.BORDER,
            padx=12,
            pady=10,
        )
        vertical = tk.Scrollbar(body, orient="vertical", command=output.yview)
        horizontal = tk.Scrollbar(body, orient="horizontal", command=output.xview)
        output.configure(yscrollcommand=vertical.set, xscrollcommand=horizontal.set)
        output.grid(row=0, column=0, sticky="nsew")
        vertical.grid(row=0, column=1, sticky="ns")
        horizontal.grid(row=1, column=0, sticky="ew")
        body.grid_rowconfigure(0, weight=1)
        body.grid_columnconfigure(0, weight=1)
        output.insert("1.0", "\n".join(lines) if lines else "아직 출력된 로그가 없습니다.")
        output.configure(state="disabled")
        output.see("end")

        footer = tk.Frame(self, bg=theme.SURFACE, padx=20, pady=12)
        footer.pack(fill="x", side="bottom")
        ActionButton(
            footer,
            text="닫기",
            fonts=fonts,
            command=self.destroy,
        ).pack(side="right")
        self.protocol("WM_DELETE_WINDOW", self.destroy)
        self.update_idletasks()
        configure_detail_dismiss(self, backdrop)


def ask_launch_profile(
    parent: tk.Misc,
    *,
    fonts: dict[str, tuple[str, int, str]],
    profile: LaunchProfileDraft | None = None,
) -> LaunchProfileDraft | None:
    """Show a profile editor and return its saved value, if any."""
    dialog = LaunchProfileDialog(parent, fonts=fonts, profile=profile)
    parent.wait_window(dialog)
    return dialog.result


def _entry(
    parent: tk.Misc,
    fonts: dict[str, tuple[str, int, str]],
    value: str,
    *,
    width: int | None = None,
) -> tk.Entry:
    entry = tk.Entry(
        parent,
        width=width or 20,
        bg=theme.SURFACE_ALT,
        fg=theme.TEXT,
        insertbackground=theme.ACCENT,
        selectbackground=theme.ACCENT_DIM,
        selectforeground=theme.TEXT,
        disabledbackground=theme.SURFACE,
        relief="flat",
        highlightthickness=1,
        highlightbackground=theme.BORDER,
        highlightcolor=theme.ACCENT,
        font=fonts["body"],
    )
    entry.insert(0, value)
    return entry
