"""Read X11 window state with xdotool; never send input or change a window."""

import subprocess


class X11Window:
    def __init__(self, tool, window, pid):
        self.tool = str(tool)
        self.window = int(window)
        self.pid = int(pid)

    def query(self, *arguments, allow_empty=False):
        result = subprocess.run(
            [self.tool, *map(str, arguments)], capture_output=True, text=True,
            timeout=5, check=False,
        )
        if result.returncode and not (allow_empty and result.returncode == 1):
            raise RuntimeError(f"X11 state query failed: {result.stderr.strip()}")
        return result.stdout.strip()

    def read(self):
        if int(self.query("getwindowpid", self.window)) != self.pid:
            raise RuntimeError("X11 window no longer belongs to the measured process")
        geometry = dict(line.split("=", 1) for line in
                        self.query("getwindowgeometry", "--shell", self.window).splitlines())
        visible = self.query("search", "--onlyvisible", "--pid", self.pid, allow_empty=True)
        # Ask for the actual input-focus ID. The default xdotool mode searches
        # for a WM_CLASS ancestor and errors on X11's None/PointerRoot sentinels,
        # which are valid while a native Windows window is active under WSLg.
        focus = int(self.query("getwindowfocus", "-f"))
        return {
            "id": self.window,
            "x": int(geometry["X"]), "y": int(geometry["Y"]),
            "width": int(geometry["WIDTH"]), "height": int(geometry["HEIGHT"]),
            "visible": str(self.window) in visible.splitlines(),
            "focused": focus == self.window,
        }

    @classmethod
    def find(cls, tool, pid):
        probe = cls(tool, 0, pid)
        windows = probe.query("search", "--onlyvisible", "--pid", pid, allow_empty=True).splitlines()
        if len(windows) != 1:
            raise RuntimeError(f"expected one visible X11 window for PID {pid}, found {len(windows)}")
        return cls(tool, int(windows[0]), pid)
