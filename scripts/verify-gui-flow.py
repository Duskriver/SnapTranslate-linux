#!/usr/bin/python3
"""Exercise the real GTK app in an isolated Xvfb / D-Bus session.

Run after `cargo build`: /usr/bin/python3 scripts/verify-gui-flow.py
Requires Python 3.11+, Xvfb, xvfb-run, dbus-run-session, python3-gi,
python3-dbus, gir1.2-atspi-2.0, gir1.2-gtk-3.0, gir1.2-gtk-4.0,
at-spi2-core, and libXtst. No real screen, credentials, or API are used.
--keep-artifacts retains application logs and isolated window screenshots.
"""

import argparse
import base64
import ctypes
import http.server
import json
import os
from pathlib import Path
import queue
import shutil
import shlex
import subprocess
import sys
import tempfile
import threading
import time
import tomllib


event_pump = lambda: None


def wait_for(check, description, timeout=8):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        event_pump()
        try:
            value = check()
            if value:
                return value
        except Exception as error:
            last_error = error
        time.sleep(0.05)
    raise AssertionError(f"Timed out: {description}; last error: {last_error}")


def require(condition, description):
    if not condition:
        raise AssertionError(description)


def settle(duration=0.12):
    """Let GTK's queued accessible actions and frame updates finish."""
    deadline = time.monotonic() + duration
    while time.monotonic() < deadline:
        event_pump()
        time.sleep(0.01)


def x_events(title="SnapTranslate · 译文", keys=(), click=None, wheel=0, focus=True, drag=None, control_click=False, hover=None):
    """Send real keyboard / mouse events only to the private Xvfb server."""
    requested_title = title
    x11 = ctypes.CDLL("libX11.so.6")
    xtst = ctypes.CDLL("libXtst.so.6")
    display_type, window_type = ctypes.c_void_p, ctypes.c_ulong
    x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
    x11.XOpenDisplay.restype = display_type
    x11.XDefaultRootWindow.argtypes = [display_type]
    x11.XDefaultRootWindow.restype = window_type
    x11.XQueryTree.argtypes = [display_type, window_type, ctypes.POINTER(window_type), ctypes.POINTER(window_type), ctypes.POINTER(ctypes.POINTER(window_type)), ctypes.POINTER(ctypes.c_uint)]
    x11.XInternAtom.argtypes = [display_type, ctypes.c_char_p, ctypes.c_int]
    x11.XInternAtom.restype = ctypes.c_ulong
    x11.XGetWindowProperty.argtypes = [display_type, window_type, ctypes.c_ulong, ctypes.c_long, ctypes.c_long, ctypes.c_int, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_void_p)]
    x11.XFree.argtypes = [ctypes.c_void_p]
    x11.XSetInputFocus.argtypes = [display_type, window_type, ctypes.c_int, ctypes.c_ulong]
    x11.XTranslateCoordinates.argtypes = [display_type, window_type, window_type, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(window_type)]
    x11.XKeysymToKeycode.argtypes = [display_type, ctypes.c_ulong]
    x11.XKeysymToKeycode.restype = ctypes.c_ubyte
    x11.XFlush.argtypes = [display_type]
    x11.XCloseDisplay.argtypes = [display_type]
    xtst.XTestFakeKeyEvent.argtypes = [display_type, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
    xtst.XTestFakeButtonEvent.argtypes = [display_type, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
    xtst.XTestFakeMotionEvent.argtypes = [display_type, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
    display = x11.XOpenDisplay(None)
    require(display, "connect to private Xvfb for Escape test")
    children = ctypes.POINTER(window_type)()
    try:
        root, parent, count = window_type(), window_type(), ctypes.c_uint()
        require(x11.XQueryTree(display, x11.XDefaultRootWindow(display), ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)), "read private Xvfb windows")
        result = None
        title_atom = x11.XInternAtom(display, b"_NET_WM_NAME", 0)
        for i in range(count.value):
            name = ctypes.c_void_p()
            actual_type, actual_format, length, remaining = ctypes.c_ulong(), ctypes.c_int(), ctypes.c_ulong(), ctypes.c_ulong()
            x11.XGetWindowProperty(display, children[i], title_atom, 0, 1024, 0, 0, ctypes.byref(actual_type), ctypes.byref(actual_format), ctypes.byref(length), ctypes.byref(remaining), ctypes.byref(name))
            if name.value:
                title = ctypes.string_at(name, length.value).decode("utf-8", errors="replace")
                x11.XFree(name)
                if title == requested_title or title.startswith(requested_title + " "):
                    result = children[i]
        require(result is not None, f"find X11 window {requested_title}")
        if focus:
            x11.XSetInputFocus(display, result, 2, 0)
        if control_click:
            xtst.XTestFakeKeyEvent(display, x11.XKeysymToKeycode(display, 0xffe3), 1, 0)
        if click or drag or hover:
            # GTK's AT-SPI WINDOW coordinates exclude client-side shadows;
            # SCREEN coordinates are intentionally unavailable in GTK4.
            origin_x, origin_y, child = ctypes.c_int(), ctypes.c_int(), window_type()
            x11.XTranslateCoordinates(display, result, x11.XDefaultRootWindow(display), 0, 0, ctypes.byref(origin_x), ctypes.byref(origin_y), ctypes.byref(child))
            frame = ctypes.c_void_p()
            frame_atom = x11.XInternAtom(display, b"_GTK_FRAME_EXTENTS", 0)
            x11.XGetWindowProperty(display, result, frame_atom, 0, 4, 0, 0, ctypes.byref(actual_type), ctypes.byref(actual_format), ctypes.byref(length), ctypes.byref(remaining), ctypes.byref(frame))
            left = top = 0
            if frame.value and length.value >= 4:
                extents = ctypes.cast(frame, ctypes.POINTER(ctypes.c_ulong))
                left, top = extents[0], extents[2]
            if frame.value:
                x11.XFree(frame)
            def move(point):
                xtst.XTestFakeMotionEvent(display, -1, int(point[0] + origin_x.value + left), int(point[1] + origin_y.value + top), 0)
            move(hover or click or drag[0])
            if not hover:
                xtst.XTestFakeButtonEvent(display, 1, 1, 0)
            if drag:
                x11.XFlush(display)
                time.sleep(0.04)
                for step in range(1, 9):
                    move(tuple(drag[0][axis] + (drag[1][axis] - drag[0][axis]) * step / 8 for axis in (0, 1)))
                    x11.XFlush(display)
                    time.sleep(0.01)
            if not hover:
                xtst.XTestFakeButtonEvent(display, 1, 0, 0)
        if control_click:
            xtst.XTestFakeKeyEvent(display, x11.XKeysymToKeycode(display, 0xffe3), 0, 0)
        for chord in keys:
            keycodes = [x11.XKeysymToKeycode(display, ord(key) if isinstance(key, str) else key) for key in chord]
            for code in keycodes:
                require(xtst.XTestFakeKeyEvent(display, code, 1, 0), "press test key")
            for code in reversed(keycodes):
                require(xtst.XTestFakeKeyEvent(display, code, 0, 0), "release test key")
        for _ in range(abs(wheel)):
            button = 4 if wheel < 0 else 5
            xtst.XTestFakeButtonEvent(display, button, 1, 0)
            xtst.XTestFakeButtonEvent(display, button, 0, 0)
        x11.XFlush(display)
    finally:
        if children:
            x11.XFree(ctypes.cast(children, ctypes.c_void_p))
        x11.XCloseDisplay(display)


def worker(root, binary, edge_only=False, ux_only=False, reading_only=False, recovery_only=False, parity_only=False, native_only=False):
    import dbus
    import dbus.service
    from dbus.mainloop.glib import DBusGMainLoop
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi, GLib

    DBusGMainLoop(set_as_default=True)
    # libatspi's object cache must be dispatched and inspected on one thread.
    # Pump D-Bus callbacks between assertions instead of racing a GLib thread.
    global event_pump
    context = GLib.MainContext.default()

    def pump():
        while context.pending():
            context.iteration(False)

    event_pump = pump
    bus = dbus.SessionBus()

    class Watcher(dbus.service.Object):
        def __init__(self):
            self.items = []
            self.name = dbus.service.BusName("org.kde.StatusNotifierWatcher", bus)
            super().__init__(self.name, "/StatusNotifierWatcher")

        @dbus.service.method("org.kde.StatusNotifierWatcher", in_signature="s", out_signature="", sender_keyword="sender")
        def RegisterStatusNotifierItem(self, service, sender=None):
            self.items.append((str(service) if not service.startswith("/") else sender,
                               str(service) if service.startswith("/") else "/StatusNotifierItem"))

        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
        def Get(self, interface, name):
            return self.GetAll(interface)[name]

        @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
        def GetAll(self, interface):
            return {"IsStatusNotifierHostRegistered": dbus.Boolean(True),
                    "ProtocolVersion": dbus.Int32(0),
                    "RegisteredStatusNotifierItems": dbus.Array([x[0] for x in self.items], signature="s")}

    watcher = Watcher()
    plans = queue.Queue()
    requests = []
    http_calls = []
    active_lock = threading.Lock()
    active_http = [0, 0]

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def do_POST(self):
            with active_lock:
                active_http[0] += 1
                active_http[1] = max(active_http)
            try:
                self.handle_translation()
            finally:
                with active_lock:
                    active_http[0] -= 1

        def handle_translation(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            http_calls.append({"body": body, "path": self.path})
            try:
                plan = plans.get(timeout=2)
            except queue.Empty:
                self.send_error(500, "No expected test response")
                return
            plan["body"] = body
            plan["path"] = self.path
            plan["authorization"] = self.headers.get("Authorization")
            requests.append(plan)
            self.close_connection = True
            if plan["mode"] in ("unauthorized", "http_error"):
                self.send_response(plan.get("status", 401))
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            try:
                def delta(text):
                    data = ("data: " + json.dumps({"choices": [{"delta": {"content": text}}]}, ensure_ascii=False) + "\n\n").encode()
                    # Deliberately split a UTF-8 character across network writes.
                    position = next((i + 1 for i, b in enumerate(data) if b >= 0xc0), len(data) // 2)
                    self.wfile.write(data[:position])
                    self.wfile.flush()
                    time.sleep(0.015)
                    self.wfile.write(data[position:])
                    self.wfile.flush()

                delta(plan["text"])
                if plan["mode"] == "truncated":
                    return  # EOF without a finish marker must be reported as failure.
                if plan["mode"] == "slow":
                    for _ in range(plan.get("ticks", 25)):
                        time.sleep(0.1)
                        delta("·")
                if plan["mode"] == "held":
                    while not plan["release"].wait(0.08):
                        delta("·")
                if plan["mode"] == "controlled":
                    while True:
                        extra = plan["chunks"].get(timeout=15)
                        if extra is None:
                            break
                        delta(extra)
                self.wfile.write(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                self.wfile.flush()
                plan["finished"] = True
            except (BrokenPipeError, ConnectionResetError):
                plan["disconnected"] = True

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    config = root / "config" / "snaptranslate"
    config.mkdir(parents=True)
    (config / "config.toml").write_text(f'target_language = "zh-CN"\nmodel = "deepseek-flash"\napi_base_url = "http://127.0.0.1:{server.server_port}"\n')
    (config / "credentials.toml").write_text('deepseek_api_key = "sk-isolated-gui-test"\n')
    (config / "credentials.toml").chmod(0o600)
    (root / "bin").mkdir()
    # The only screenshot provider returns this generated fixture, never real pixels.
    fixture = root / "fixture.png"
    fixture.write_bytes(base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j2ioAAAAASUVORK5CYII="))
    maim = root / "bin" / "maim"
    maim.write_text('#!/usr/bin/python3\nimport os, pathlib, sys, time\nr=pathlib.Path(os.environ["SNAP_GUI_TEST_ROOT"])\nwith (r/"capture-calls").open("a") as log: log.write("capture\\n")\nif (r/"cancel-capture").exists(): sys.exit(1)\nif (r/"slow-capture").exists():\n (r/"selector.pid").write_text(str(os.getpid()))\n time.sleep(30)\npathlib.Path(sys.argv[-1]).write_bytes((r/"fixture.png").read_bytes())\n')
    maim.chmod(0o755)
    app_env = dict(os.environ, PATH=str(root / "bin"), SNAP_GUI_TEST_ROOT=str(root))
    processes = []
    logfiles = []
    launcher = next((Path(p) for p in ["/usr/libexec/at-spi-bus-launcher", "/usr/lib/at-spi2-core/at-spi-bus-launcher"] if Path(p).exists()), None)
    require(launcher is not None, "at-spi-bus-launcher is required")
    a11y_log = (root / "accessibility.log").open("w")
    logfiles.append(a11y_log)
    a11y = subprocess.Popen([str(launcher), "--launch-immediately"], stdout=a11y_log, stderr=subprocess.STDOUT)
    processes.append(a11y)
    wait_for(lambda: bus.name_has_owner("org.a11y.Bus"), "isolated accessibility bus")
    Atspi.init()

    def spawn(*args):
        log = (root / f"app-{len(processes)}.log").open("w")
        logfiles.append(log)
        process = subprocess.Popen([str(binary), *args], env=app_env, stdout=log, stderr=subprocess.STDOUT)
        processes.append(process)
        return process

    window_titles = {}

    def walk(node, refresh=True):
        # GTK rebuilds accessible objects when a capture hides/unmaps a window.
        # Resolve saved top-level proxies by stable application window title.
        title = window_titles.get(hash(node)) if refresh else None
        if title:
            node = next((w for w in windows() if w.get_name() == title), node)
        yield node
        for i in range(node.get_child_count()):
            child = node.get_child_at_index(i)
            if child:
                yield from walk(child, False)

    def windows():
        desktop = Atspi.get_desktop(0)
        result = []
        for i in range(desktop.get_child_count()):
            try:
                app = desktop.get_child_at_index(i)
                if app and app.get_process_id() in [p.pid for p in processes if p.poll() is None]:
                    for j in range(app.get_child_count()):
                        window = app.get_child_at_index(j)
                        if window and window.get_state_set().contains(Atspi.StateType.SHOWING):
                            window_titles[hash(window)] = window.get_name()
                            result.append(window)
            except GLib.Error:
                # The registry can retain a proxy after its process has exited.
                # One dead application must not hide windows of the next one.
                continue
        return result

    def text(window):
        values = []
        for node in walk(window):
            if node.get_state_set().contains(Atspi.StateType.VISIBLE):
                values.append(node.get_name() or "")
                interface = node.get_text_iface()
                if interface:
                    values.append(Atspi.Text.get_text(interface, 0, -1))
        return "\n".join(values)

    def find_window(fragment):
        return next((w for w in windows() if fragment in text(w)), None)

    def close(window):
        for node in walk(window):
            if node.get_name() in ("Close", "关闭"):
                action = node.get_action_iface()
                if action:
                    require(Atspi.Action.do_action(action, 0), "activate window close button")
                    return
        raise AssertionError("No accessible Close button in window: " + text(window))

    def click(window, label):
        def target():
            for current_window in [window] if window is not None else windows():
                for node in walk(current_window):
                    if node.get_name() == label:
                        action = node.get_action_iface()
                        if action and Atspi.Action.get_n_actions(action) > 0:
                            return action
            return None
        action = wait_for(target, f"actionable button {label}")
        require(Atspi.Action.do_action(action, 0), f"activate {label}")
        settle()

    def named(window, label):
        return next((node for node in walk(window) if node.get_name() == label), None)

    def input_node(window, label):
        return next((node for node in walk(window) if node.get_name() == label and "EditableText" in node.get_interfaces()), None)

    def shown(window, label):
        return any(node.get_name() == label and node.get_state_set().contains(Atspi.StateType.SHOWING) for node in walk(window))

    def visible_toast_text(window):
        # Inspect the transient Toast itself, not the persistent status label.
        return "\n".join(text(node.get_parent()) for node in walk(window) if node.get_name() == "Dismiss" and node.get_state_set().contains(Atspi.StateType.SHOWING))

    def node_text(node):
        return Atspi.Text.get_text(node, 0, -1)

    def screenshot(window, filename):
        rect = Atspi.Component.get_extents(window, Atspi.CoordType.SCREEN)
        # GTK3's tiny screenshot helper is kept in a separate process so the
        # harness can use GTK4 for the clipboard. DISPLAY is the private Xvfb.
        code = """import gi, sys
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk
root = Gdk.get_default_root_window()
x,y,width,height = map(int, sys.argv[2:])
image = Gdk.pixbuf_get_from_window(root, x, y, width, height)
if image is None: raise RuntimeError('Cannot capture private Xvfb window')
image.savev(sys.argv[1], 'png', [], [])
"""
        subprocess.run([sys.executable, "-c", code, str(root / filename), str(rect.x), str(rect.y), str(rect.width), str(rect.height)], check=True, timeout=5)

    def viewport_pixels(window, content, name, bottom_inset=48):
        from gi.repository import GdkPixbuf
        x_events(title=window.get_name(), hover=(400, 15))
        settle(.18)
        screenshot(window, name + ".png")
        image = GdkPixbuf.Pixbuf.new_from_file(str(root / (name + ".png")))
        box = Atspi.Component.get_extents(content, Atspi.CoordType.WINDOW)
        x0, y0 = max(0, box.x + 20), max(0, box.y + 8)
        width = min(box.width - 40, image.get_width() - x0)
        height = min(box.height - bottom_inset - 8, image.get_height() - y0)
        require(width > 0 and height > 0, "readable viewport has rendered pixels")
        channels, stride = image.get_n_channels(), image.get_rowstride()
        data = image.get_pixels()
        rows = [bytes(data[(y0+y)*stride+x0*channels:(y0+y)*stride+(x0+width)*channels]) for y in range(height)]
        return b"".join(rows), width, height, channels, x0, y0

    def pixel_difference(a, b):
        require(a[1:4] == b[1:4], "viewport geometry stays fixed")
        channels = a[3]
        changed = sum(any(abs(a[0][i+c] - b[0][i+c]) > 10 for c in range(3)) for i in range(0, len(a[0]), channels))
        return changed / (a[1] * a[2])

    def last_text_point(window, content):
        data, width, height, channels, x0, y0 = viewport_pixels(window, content, "link-rendered", 8)
        background = data[(width - 1)*channels:(width - 1)*channels+3]
        ink_rows = []
        for y in range(height):
            count = sum(any(abs(data[(y*width+x)*channels+c]-background[c]) > 35 for c in range(3)) for x in range(width))
            if count >= 4: ink_rows.append(y)
        require(ink_rows, "rendered text has visible ink")
        return x0 + 70, y0 + ink_rows[-1] - 6

    def pointer(window, node, focus=True):
        try:
            Atspi.Component.scroll_to(node, Atspi.ScrollType.ANYWHERE)
        except Exception:
            pass
        event_pump()
        rect = Atspi.Component.get_extents(node, Atspi.CoordType.WINDOW)
        require(rect.width > 0 and rect.height > 0, "control has visible bounds")
        x_events(title=window.get_name(), click=(rect.x + min(rect.width / 2, 50), rect.y + min(rect.height / 2, 12)), focus=focus)

    clipboard = None

    def init_clipboard():
        import gi
        gi.require_version("Gtk", "4.0")
        gi.require_version("Gdk", "4.0")
        from gi.repository import Gdk, Gtk
        Gtk.init()
        nonlocal clipboard
        clipboard = Gdk.Display.get_default().get_clipboard()

    def clipboard_text():
        result = []
        def ready(source, response, *_):
            result.append(source.read_text_finish(response))
        clipboard.read_text_async(None, ready, None)
        wait_for(lambda: result, "read actual X11 clipboard")
        return result[0] or ""

    def focus_input(window, node):
        x_events(title=window.get_name())
        try:
            Atspi.Component.scroll_to(node, Atspi.ScrollType.ANYWHERE)
            focused = Atspi.Component.grab_focus(node)
        except GLib.Error:
            focused = False
        if not focused:
            pointer(window, node)
        settle()

    def entry_text(window, label):
        node = wait_for(lambda: input_node(window, label), f"find input {label}")
        focus_input(window, node)
        # GTK 4.14 can expose EditableText without implementing Text.get_text.
        # Read through the real selection/clipboard instead of trusting that API.
        sentinel = "private-input-copy-sentinel"
        clipboard.set(sentinel)
        x_events(title=window.get_name(), keys=[(0xffe3, "a"), (0xffe3, "c")])
        settle(.2)
        value = clipboard_text()
        return "" if value == sentinel else value

    def replace_text(window, label, value, masked=False):
        node = wait_for(lambda: input_node(window, label), f"find input {label}")
        focus_input(window, node)
        clipboard.set(value)
        chords = [(0xffe3, "a"), (0xffe3, "v")] if value else [(0xffe3, "a"), (0xff08,)]
        x_events(title=window.get_name(), keys=chords)
        settle()
        if not masked:
            wait_for(lambda: entry_text(window, label) == value, f"input {label} receives fixture text")
        # Password contents intentionally remain unreadable. Recovery tests verify
        # the exact synthetic key in the subsequent local HTTP Authorization header.

    def choose_language(window, index):
        node = wait_for(lambda: named(window, "目标语言"), "language selector")
        # ComboRow's popup has its own focus; forcing focus back to the parent
        # window before keyboard navigation would dismiss it.
        action = node.get_action_iface()
        if action and Atspi.Action.get_n_actions(action) > 0:
            Atspi.Action.do_action(action, 0)
        else:
            pointer(window, node)
        labels = ["简体中文", "繁體中文", "English", "日本語", "한국어", "自定义"]
        wait_for(lambda: next((item for item in walk(window) if item.get_name() == labels[index] and item.get_state_set().contains(Atspi.StateType.SHOWING)), None), "language popup option")
        x_events(title=window.get_name(), focus=False, keys=[(0xff52,)] * len(labels) + [(0xff54,)] * index + [(0xff0d,)])
        event_pump()

    def plan(mode, text="", **extra):
        response = dict(mode=mode, text=text, **extra)
        plans.put(response)
        return response

    def command(*args):
        process = spawn(*args)
        wait_for(lambda: process.poll() is not None, f"secondary command returns: {args}", timeout=8)
        require(process.returncode == 0, f"secondary command succeeded: {args}")

    def tray(label):
        service, path = wait_for(lambda: next((item for item in reversed(watcher.items) if bus.name_has_owner(item[0])), None), "live tray registration")
        item = bus.get_object(service, path)
        menu_path = item.Get("org.kde.StatusNotifierItem", "Menu", dbus_interface="org.freedesktop.DBus.Properties")
        menu = bus.get_object(service, menu_path)
        _, layout = menu.GetLayout(0, -1, [], dbus_interface="com.canonical.dbusmenu")

        def search(entry):
            if str(entry[1].get("label", "")) == label:
                return int(entry[0])
            for child in entry[2]:
                found = search(child)
                if found is not None:
                    return found
            return None

        item_id = search(layout)
        require(item_id is not None, f"tray menu contains {label}")
        menu.Event(item_id, "clicked", dbus.Int32(0, variant_level=1), dbus.UInt32(0), dbus_interface="com.canonical.dbusmenu")
        # The application drains tray actions every 100 ms. Let the requested
        # action run before responding to an already-open confirmation dialog.
        settle(0.2)

    def standard_flows():
        success = plan("success", "冷启动译文：中文完整。")
        primary = spawn("capture")
        window = wait_for(lambda: find_window(success["text"]), "cold capture renders translated Chinese")
        wait_for(lambda: "正在翻译" not in text(window), "success clears loading state")
        require(primary.poll() is None, "cold capture stays alive until the result closes")
        require(success["path"] == "/chat/completions", "configured API base URL is used")
        require(success["authorization"] == "Bearer sk-isolated-gui-test", "only fixture credentials were sent")
        close(window)
        wait_for(lambda: not windows(), "cold capture result closes")
        require(primary.poll() is None, "cold capture remains available in the tray")
        tray("退出 SnapTranslate")
        require(primary.wait(timeout=5) == 0, "cold capture process exits via tray Quit")
        print("PASS cold capture, local endpoint, UTF-8 streaming, resident lifetime", flush=True)

        resident = spawn()
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "resident settings opens")
        close(settings)
        wait_for(lambda: not windows(), "settings closes")
        require(resident.poll() is None, "closing settings preserves resident process")
        again = plan("success", "SECOND_CAPTURE_OK")
        command("capture", "--lang", "en")
        second_window = wait_for(lambda: find_window(again["text"]), "secondary capture reaches existing app")
        request_text = json.dumps(again["body"], ensure_ascii=False)
        require("翻译成en" in request_text, "secondary --lang override reaches the HTTP request")
        close(second_window)
        print("PASS settings close keeps tray resident; secondary capture forwards language", flush=True)

        denied = plan("unauthorized")
        command("capture")
        error_window = wait_for(lambda: find_window("错误"), "HTTP 401 is visible in result window")
        require("API" in text(error_window) or "密钥" in text(error_window), "401 error explains credentials")
        close(error_window)
        partial = plan("truncated", "PARTIAL_TRANSLATION")
        command("capture")
        partial_window = wait_for(lambda: find_window(partial["text"]), "partial translation renders")
        wait_for(lambda: "错误" in text(partial_window), "early EOF displays an error with partial text")
        close(partial_window)
        print("PASS HTTP errors and interrupted streams are visible; partial text retained", flush=True)

        canceled = plan("slow", "CANCEL_THIS_WINDOW", ticks=150)
        command("capture")
        canceled_window = wait_for(lambda: find_window(canceled["text"]), "first concurrent result")
        kept = plan("slow", "KEEP_THIS_WINDOW", ticks=20)
        command("capture")
        kept_window = wait_for(lambda: find_window(kept["text"]), "second concurrent result")
        close(canceled_window)
        wait_for(lambda: canceled.get("disconnected"), "closing one window cancels its HTTP connection", timeout=4)
        wait_for(lambda: kept.get("finished"), "other request continues to completion")
        wait_for(lambda: "正在翻译" not in text(kept_window), "other window completes normally")
        require("错误" not in text(kept_window), "other result is unaffected")
        close(kept_window)
        print("PASS independent windows: close cancels only its own request", flush=True)

        before = len(requests)
        previous_calls = (root / "capture-calls").read_text().count("capture")
        (root / "cancel-capture").touch()
        command("capture")
        wait_for(lambda: (root / "capture-calls").read_text().count("capture") > previous_calls, "canceled selector was invoked")
        time.sleep(0.25)
        require(len(requests) == before and not windows(), "capture cancellation produces no HTTP request or error window")
        (root / "cancel-capture").unlink()
        require(resident.poll() is None, "resident survives canceled selection")
        print("PASS canceled screenshot does not translate or show an error", flush=True)

        tray("设置...")
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "tray Settings action")
        close(settings)
        tray_result = plan("success", "TRAY_CAPTURE_OK")
        tray("截屏翻译")
        window = wait_for(lambda: find_window(tray_result["text"]), "tray Capture action")
        close(window)
        tray("退出 SnapTranslate")
        require(resident.wait(timeout=5) == 0, "tray Quit exits cleanly")
        print("PASS tray registration and Settings / Capture / Quit actions", flush=True)

    def ux_flows():
        init_clipboard()
        resident = spawn()
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "interactive settings")
        screenshot(settings, "settings.png")
        click(settings, "复制命令")
        wait_for(lambda: "截屏命令已复制" in text(settings), "shortcut copy feedback")
        require(shlex.split(clipboard_text()) == [str(binary.resolve()), "capture"], "clipboard contains executable absolute capture command")
        print("PASS shortcut command copy feedback and actual clipboard payload", flush=True)

        config_path = config / "config.toml"
        choose_language(settings, 2)  # English
        click(settings, "保存设置")
        wait_for(lambda: tomllib.loads(config_path.read_text())["target_language"] == "en", "common language is saved")
        choose_language(settings, 5)  # Custom
        replace_text(settings, "自定义目标语言", "fr")
        click(settings, "保存设置")
        wait_for(lambda: tomllib.loads(config_path.read_text())["target_language"] == "fr", "custom language is saved")
        saved_config = config_path.read_bytes()
        pointer(settings, named(settings, "高级设置"))
        replace_text(settings, "模型名称", "")
        click(settings, "保存设置")
        wait_for(lambda: "模型名称不能为空" in text(settings), "invalid input has actionable validation")
        require(config_path.read_bytes() == saved_config, "invalid settings never overwrite the saved configuration")
        click(settings, "取消修改")
        wait_for(lambda: entry_text(settings, "模型名称") == "deepseek-flash", "cancel restores model")
        credentials = config / "credentials.toml"
        saved_key = credentials.read_bytes()
        click(settings, "删除密钥")
        wait_for(lambda: named(settings, "撤销删除"), "key deletion is staged")
        require(credentials.read_bytes() == saved_key, "staging deletion leaves fixture key intact")
        click(settings, "撤销删除")
        require(credentials.read_bytes() == saved_key, "undo deletion preserves fixture key")
        click(settings, "删除密钥")
        click(settings, "取消修改")
        wait_for(lambda: named(settings, "删除密钥"), "cancel clears staged key deletion")
        require(credentials.read_bytes() == saved_key, "cancel staged deletion preserves fixture key")
        replace_text(settings, "模型名称", "discard-this-draft")
        close(settings)
        confirm = wait_for(lambda: find_window("放弃修改"), "unsaved-close confirmation")
        click(confirm, "放弃修改")
        wait_for(lambda: not windows(), "discard closes settings")
        command()
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "reopen persisted settings")
        require(entry_text(settings, "模型名称") == "deepseek-flash", "discarded draft does not survive reopening")
        require(tomllib.loads(config_path.read_text())["target_language"] == "fr", "saved custom language survives reopening")
        require(config_path.read_bytes() == saved_config and credentials.read_bytes() == saved_key, "discard leaves both stores unchanged")
        close(settings)
        wait_for(lambda: not windows(), "clean settings closes without confirmation")
        print("PASS common/custom languages, validation, cancel, staged key undo, and close/reopen", flush=True)

        markdown = "# Copy heading\n\n**Bold phrase** and `code`.\n\n- first item\n- second item\n\n```rust\nlet value = 42;\n```\n"
        plan("success", markdown)
        command("capture")
        result = wait_for(lambda: find_window("Copy heading"), "rich result displays")
        wait_for(lambda: "翻译完成" in text(result), "rich result completes")
        content = wait_for(lambda: named(result, "译文内容"), "native result TextView")
        visible_text = node_text(content)
        screenshot(result, "result.png")
        require("**" not in visible_text and "`" not in visible_text and "# Copy" not in visible_text, "completed result removes Markdown syntax")
        click(result, "复制译文")
        wait_for(lambda: "已复制" in text(result), "copy result feedback")
        require(clipboard_text() == visible_text, "copy button copies displayed plain text")

        pointer(result, content)
        x_events(title=result.get_name(), keys=[(0xffe3, 0xff50), (0xffe1, 0xff57), (0xffe3, "c")])
        wait_for(lambda: clipboard_text() == "Copy heading", "Ctrl+C copies only the selected heading")
        x_events(title=result.get_name(), keys=[(0xffe3, 0xffe1, "c")])
        wait_for(lambda: clipboard_text() == visible_text, "Ctrl+Shift+C copies all text despite selection")
        x_events(title=result.get_name(), keys=[(0xffe3, "a"), (0xffe3, "c")])
        wait_for(lambda: clipboard_text() == visible_text, "Ctrl+A and Ctrl+C copy the whole result")
        x_events(title=result.get_name(), keys=[(0xffe3, 0xff50)])
        settle()
        try:
            start = Atspi.Text.get_character_extents(content, 0, Atspi.CoordType.WINDOW)
            end = Atspi.Text.get_character_extents(content, 4, Atspi.CoordType.WINDOW)
            drag = ((start.x, start.y + start.height / 2), (end.x + end.width, end.y + end.height / 2))
            precise_selection = True
        except GLib.Error:
            box = Atspi.Component.get_extents(content, Atspi.CoordType.WINDOW)
            drag = ((box.x + 16, box.y + 18), (box.x + 300, box.y + 18))
            precise_selection = False
        clipboard.set("private-drag-copy-sentinel")
        x_events(title=result.get_name(), drag=drag)
        x_events(title=result.get_name(), keys=[(0xffe3, "c")])
        if precise_selection:
            wait_for(lambda: Atspi.Text.get_n_selections(content) > 0, "native mouse drag creates a text selection")
            selected = Atspi.Text.get_selection(content, 0)
            selected_text = visible_text[selected.start_offset:selected.end_offset]
        else:
            selected_text = "Copy heading"
        require(bool(selected_text) and selected_text != visible_text, "mouse drag selects part of the document")
        wait_for(lambda: clipboard_text() == selected_text, "Ctrl+C copies actual mouse-drag selection")
        print("PASS plain-text copy, keyboard selection, select-all, and native mouse-drag copy", flush=True)

        click(result, "设置")
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "result Settings action")
        replace_text(settings, "模型名称", "unsaved-model-blocks-capture")
        before = len(requests)
        click(result, "再次截图")
        wait_for(lambda: "请先保存或取消修改后再截图" in text(settings), "result action blocks capture with unsaved settings")
        command("capture")
        settle()
        require(len(requests) == before, "unsaved settings block both result and remote capture without HTTP")
        x_events(title=settings.get_name(), keys=[(0xffe3, "q")])
        confirm = wait_for(lambda: find_window("放弃修改"), "Ctrl+Q asks before discarding settings")
        click(None, "继续编辑")
        wait_for(lambda: find_window("放弃修改") is None, "continue editing closes confirmation")
        require(resident.poll() is None, "continue editing cancels Ctrl+Q")
        click(settings, "取消修改")
        close(settings)
        command("config", "set-language", "en")
        require(tomllib.loads(config_path.read_text())["target_language"] == "en", "CLI changes default language for later captures")
        again = plan("slow", "RESULT_ACTION_CAPTURE", ticks=150)
        click(result, "再次截图")
        second = wait_for(lambda: find_window(again["text"]), "result Capture Again action")
        require("翻译成fr" in json.dumps(again["body"], ensure_ascii=False), "Capture Again preserves original result language after default changes")
        close(second)
        wait_for(lambda: again.get("disconnected"), "closing second result cancels only its request")
        require("Copy heading" in text(result), "original result remains readable")
        close(result)
        print("PASS dirty capture guards, Ctrl+Q cancellation, result actions and preserved language", flush=True)

        reading_flows(resident)

        command()
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "settings for pending-close Quit regression")
        replace_text(settings, "模型名称", "discard-on-quit")
        close(settings)
        confirm = wait_for(lambda: find_window("放弃修改"), "dirty close confirmation")
        tray("退出 SnapTranslate")
        click(None, "继续编辑")
        wait_for(lambda: find_window("放弃修改") is None, "pending Quit confirmation is dismissed")
        require(resident.poll() is None, "continue editing clears pending Quit intention")
        close(settings)
        confirm = wait_for(lambda: find_window("放弃修改"), "dirty close can be requested again")
        tray("退出 SnapTranslate")
        click(None, "放弃修改")
        require(resident.wait(timeout=5) == 0, "UX test application exits cleanly")
        print("PASS pending-close dialog upgrades to Quit and Continue Editing clears Quit", flush=True)

    def reading_flows(resident):
        chunks = queue.Queue()
        initial = "\n\n".join(f"Initial line {i:03d}" for i in range(45)) + "\n\nTAIL_INITIAL"
        streaming = plan("controlled", initial, chunks=chunks)
        command("capture")
        result = wait_for(lambda: find_window("TAIL_INITIAL"), "long streaming result")
        content = wait_for(lambda: named(result, "译文内容"), "streaming TextView")

        def assert_at_end(name):
            before = viewport_pixels(result, content, name + "-current")
            x_events(title=result.get_name(), keys=[(0xffe3, 0xff57)])
            after = viewport_pixels(result, content, name + "-end")
            require(pixel_difference(before, after) < .003, "automatic follow already displays the end of the stream")

        wait_for(lambda: "TAIL_INITIAL" in node_text(content), "initial stream arrives")
        assert_at_end("initial-follow")
        pointer(result, content)
        x_events(title=result.get_name(), wheel=-12)
        wait_for(lambda: shown(result, "回到底部"), "scrolling up reveals return-to-bottom action")
        before = viewport_pixels(result, content, "reading-before-append")
        chunks.put("\n\n" + "\n\n".join(f"New line {i:03d}" for i in range(12)) + "\n\nTAIL_ADDED")
        wait_for(lambda: "TAIL_ADDED" in node_text(content), "more streamed text arrives while scrolled up")
        after = viewport_pixels(result, content, "reading-after-append")
        require(pixel_difference(before, after) < .003, "incoming text preserves the rendered reading position")
        click(result, "回到底部")
        wait_for(lambda: not shown(result, "回到底部"), "return-to-bottom resumes following")
        chunks.put("\n\nTAIL_FOLLOWING_AGAIN")
        wait_for(lambda: "TAIL_FOLLOWING_AGAIN" in node_text(content), "more text arrives after returning to bottom")
        assert_at_end("resumed-follow")
        pointer(result, content)
        x_events(title=result.get_name(), wheel=-12)
        wait_for(lambda: shown(result, "回到底部"), "user scrolls up again before formatting completes")
        before = viewport_pixels(result, content, "reading-before-completion")
        chunks.put(None)
        wait_for(lambda: "翻译完成" in text(result), "controlled stream completes")
        after = viewport_pixels(result, content, "reading-after-completion")
        require(pixel_difference(before, after) < .003, "final formatting preserves the rendered reading position")
        x_events(title=result.get_name(), keys=[(0xffe3, "w")])
        wait_for(lambda: not windows(), "Ctrl+W closes result")
        print("PASS streaming follow, preserved reading position through completion, and return-to-bottom", flush=True)


    def recovery_flows():
        init_clipboard()

        def capture_count():
            path = root / "capture-calls"
            return path.read_text().count("capture") if path.exists() else 0

        def image_url(response):
            return next(part["image_url"]["url"] for message in response["body"]["messages"] if isinstance(message.get("content"), list) for part in message["content"] if part.get("type") == "image_url")

        def result_count():
            return sum(window.get_name().startswith("SnapTranslate · 译文") for window in windows())

        failure = plan("unauthorized")
        resident = spawn("capture")
        result = wait_for(lambda: find_window("检查 API Key"), "401 offers a credential recovery action")
        original_title = result.get_name()
        original_image = image_url(failure)
        initial_captures = capture_count()
        require(result_count() == 1, "capture creates one result window")
        screenshot(result, "recovery-401.png")
        click(result, "检查 API Key")
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "401 recovery opens settings")
        wait_for(lambda: "检查并保存 API Key" in visible_toast_text(settings), "401 shows current credential guidance in its Toast", timeout=3)
        key_input = wait_for(lambda: input_node(settings, "API Key"), "credential recovery target")
        require(key_input.get_state_set().contains(Atspi.StateType.SHOWING), "credential field is brought into view")
        x_events(title=settings.get_name())  # Xvfb has no window manager to activate restored windows.
        wait_for(lambda: any(node.get_state_set().contains(Atspi.StateType.FOCUSED) for node in walk(key_input)), "credential recovery focuses the API Key field")
        replace_text(settings, "API Key", "sk-recovered-fixture", masked=True)
        model_input = input_node(settings, "模型名称")
        if model_input is None or not model_input.get_state_set().contains(Atspi.StateType.SHOWING):
            pointer(settings, named(settings, "高级设置"))
        replace_text(settings, "模型名称", "mock-vision-retry")
        replace_text(settings, "API 基础地址", f"http://127.0.0.1:{server.server_port}/reconfigured")
        before = len(http_calls)
        click(result, "原图重试")
        wait_for(lambda: "请先保存或取消" in text(settings), "dirty settings block screenshot retry")
        settle()
        require(len(http_calls) == before and capture_count() == initial_captures, "dirty retry creates neither request nor new screenshot")
        click(settings, "保存设置")
        wait_for(lambda: "设置已保存，请返回译文窗口点击原图重试" in text(settings), "recovery save explains the next action")
        close(settings)

        endpoint_error = plan("http_error", status=404)
        click(result, "原图重试")
        wait_for(lambda: "检查模型与接口" in text(result), "404 offers endpoint / model recovery")
        require(result.get_name() == original_title and result_count() == 1, "retry keeps the original window and number")
        require(capture_count() == initial_captures and image_url(endpoint_error) == original_image, "retry reuses the original image without maim")
        require(endpoint_error["authorization"] == "Bearer sk-recovered-fixture", "retry reloads saved credentials")
        require(endpoint_error["body"]["model"] == "mock-vision-retry", "retry reloads saved model")
        require(endpoint_error["path"] == "/reconfigured/chat/completions", "retry reloads saved API base URL")
        click(result, "检查模型与接口")
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "404 recovery opens settings")
        wait_for(lambda: "检查并保存模型与接口地址" in visible_toast_text(settings) and "检查并保存 API Key" not in visible_toast_text(settings), "404 replaces stale credential Toast with model / endpoint guidance", timeout=3)
        model_input = input_node(settings, "模型名称")
        require(model_input.get_state_set().contains(Atspi.StateType.SHOWING), "endpoint recovery expands advanced settings and reveals model")
        require(input_node(settings, "API 基础地址").get_state_set().contains(Atspi.StateType.SHOWING), "endpoint recovery reveals API base URL")
        x_events(title=settings.get_name())
        settle()
        clipboard.set("unmodified-focus-sentinel")
        x_events(title=settings.get_name(), keys=[(0xffe3, "a"), (0xffe3, "c")])
        wait_for(lambda: clipboard_text() == f"http://127.0.0.1:{server.server_port}/reconfigured", "endpoint recovery focuses API base URL for keyboard editing")
        screenshot(settings, "recovery-settings.png")
        close(settings)
        print("PASS 401/404 recovery, dirty retry guard, and latest key/model/endpoint on the same screenshot", flush=True)

        partial = plan("slow", "STOPPED_PARTIAL_TRANSLATION", ticks=150)
        before = len(http_calls)
        retry = next(node for node in walk(result) if node.get_name() == "原图重试" and Atspi.Action.get_n_actions(node) > 0)
        require(Atspi.Action.do_action(retry, 0), "start retry")
        # Queue a second activation before GTK has processed the first one.
        # The app must guard the attempt even if the second event is delivered.
        Atspi.Action.do_action(retry, 0)
        wait_for(lambda: partial["text"] in text(result), "retry streams into the original window")
        settle(0.25)
        require(len(http_calls) == before + 1, "rapid repeated retry starts only one HTTP request")
        require(capture_count() == initial_captures and image_url(partial) == original_image, "rapid retry still reuses one screenshot")
        click(result, "停止翻译")
        wait_for(lambda: partial.get("disconnected"), "Stop disconnects the HTTP stream", timeout=4)
        wait_for(lambda: "已停止翻译" in text(result), "Stop has a visible stopped state")
        require(result.get_name() == original_title and result_count() == 1, "Stop keeps the result window")
        content = named(result, "译文内容")
        stopped_text = node_text(content)
        require(partial["text"] in stopped_text, "Stop retains received translation")
        click(result, "复制译文")
        wait_for(lambda: "已复制" in text(result), "stopped result shows copy feedback")
        require(clipboard_text() == node_text(content), "stopped partial translation remains copyable")
        screenshot(result, "recovery-stopped.png")

        replacement = plan("success", "REPLACEMENT_TRANSLATION")
        click(result, "原图重试")
        wait_for(lambda: replacement["text"] in node_text(content) and "翻译完成" in text(result), "stopped request can be retried successfully")
        require(partial["text"] not in node_text(content), "retry replaces old text instead of appending")
        require(result.get_name() == original_title and capture_count() == initial_captures, "successful retry keeps the original result identity and screenshot")
        screenshot(result, "recovery-success.png")
        print("PASS Stop preserves copyable partial text; guarded same-window retry replaces it", flush=True)

        other = plan("slow", "OTHER_RESULT_CONTINUES", ticks=25)
        command("capture")
        other_window = wait_for(lambda: find_window(other["text"]), "second independent result")
        closing_retry = plan("slow", "CLOSING_RETRY_ONLY", ticks=150)
        click(result, "原图重试")
        wait_for(lambda: closing_retry["text"] in text(result), "original result retries while another result is running")
        require(capture_count() == initial_captures + 1, "only the explicit second capture invokes maim")
        require(image_url(closing_retry) == original_image, "the retry still uses the first captured image")
        close(result)
        wait_for(lambda: closing_retry.get("disconnected"), "closing retried result cancels that request", timeout=4)
        wait_for(lambda: other.get("finished") and "翻译完成" in text(other_window), "other result completes independently")
        require("错误" not in text(other_window), "other result is unaffected by retry cancellation")
        close(other_window)
        tray("退出 SnapTranslate")
        require(resident.wait(timeout=5) == 0, "recovery test application exits")
        print("PASS closing a retried request cancels only its own window", flush=True)

    def parity_flows():
        init_clipboard()

        def image_url(response):
            return next(part["image_url"]["url"] for message in response["body"]["messages"] if isinstance(message.get("content"), list) for part in message["content"] if part.get("type") == "image_url")
        resident = spawn()
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "parity settings")
        wait_for(lambda: "当前桌面未提供系统快捷键接口" in text(settings), "unsupported shortcut portal has honest fallback")
        close(settings)
        active_http[1] = 0
        a = plan("held", "ACTIVE_A", release=threading.Event())
        command("capture")
        wa = wait_for(lambda: find_window("ACTIVE_A"), "first active request")
        b = plan("held", "ACTIVE_B", release=threading.Event())
        command("capture")
        wb = wait_for(lambda: find_window("ACTIVE_B"), "second active request")
        before = len(http_calls)
        command("capture")
        queued = wait_for(lambda: find_window("等待翻译"), "third capture queues")
        screenshot(queued, "queued.png")
        click(queued, "取消等待")
        wait_for(lambda: "已取消等待" in text(queued), "cancel waiting state")
        require(len(http_calls) == before, "cancelled queue item never uploads")
        close(queued)
        command("capture")
        queued = wait_for(lambda: find_window("等待翻译"), "close waiting window")
        close(queued)
        settle()
        require(len(http_calls) == before, "closed queue item never uploads")
        c = plan("success", "ADMITTED_C")
        command("capture")
        queued = wait_for(lambda: find_window("等待翻译"), "next queued capture")
        a["release"].set()
        wc = wait_for(lambda: find_window("ADMITTED_C"), "released slot admits next task")
        require(active_http[1] <= 2, "network concurrency remains at most two")
        close(wa); close(wc)
        b["release"].set()
        wait_for(lambda: "翻译完成" in text(wb), "remaining request completes")
        print("PASS two network slots, queue admission, stop waiting, and close waiting", flush=True)

        # Keep an existing result and settings visible during the next capture.
        command()
        settings = wait_for(lambda: find_window("DeepSeek API Key"), "settings before selection")
        (root / "slow-capture").touch()
        (root / "selector.pid").unlink(missing_ok=True)
        before = len(http_calls)
        command("capture")
        pid = int(wait_for(lambda: (root / "selector.pid").read_text() if (root / "selector.pid").exists() else None, "pending selector"))
        require(not windows(), "all own settings/results are hidden while selecting")
        command("capture")
        wait_for(lambda: not Path(f"/proc/{pid}").exists(), "second capture cancels and reaps selector")
        wait_for(lambda: find_window("ACTIVE_B") and find_window("DeepSeek API Key"), "cancel restores existing result and settings")
        require(len(http_calls) == before, "cancelled selection has no translation or error")
        (root / "slow-capture").unlink()
        close(settings)
        close(wb)
        tray("退出 SnapTranslate")
        require(resident.wait(timeout=5) == 0, "parity resident exits")
        print("PASS selection toggle cancellation, window hiding, and restoration", flush=True)

        class Request(dbus.service.Object):
            def __init__(self, path):
                self.closed = False
                super().__init__(bus, path)
            @dbus.service.signal("org.freedesktop.portal.Request", signature="ua{sv}")
            def Response(self, code, results): pass
            @dbus.service.method("org.freedesktop.portal.Request", in_signature="", out_signature="")
            def Close(self): self.closed = True
        class Session(dbus.service.Object):
            def __init__(self, path):
                self.closed = False
                super().__init__(bus, path)
            @dbus.service.signal("org.freedesktop.portal.Session", signature="a{sv}")
            def Closed(self, options): pass
            @dbus.service.method("org.freedesktop.portal.Session", in_signature="", out_signature="")
            def Close(self): self.closed = True; self.Closed({})
        class Portal(dbus.service.Object):
            def __init__(self):
                self.name = dbus.service.BusName("org.freedesktop.portal.Desktop", bus)
                self.objects = []
                self.bind_status = 1
                self.saved = False
                self.trigger = "Ctrl+Alt+Y"
                self.session = None
                self.screenshot_mode = None
                self.pending = None
                super().__init__(self.name, "/org/freedesktop/portal/desktop")
            @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
            def Get(self, interface, name): return dbus.UInt32(2)
            @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
            def GetAll(self, interface): return {"version": dbus.UInt32(2)}
            @dbus.service.method("org.freedesktop.host.portal.Registry", in_signature="sa{sv}", out_signature="")
            def Register(self, app_id, options): pass
            def request(self, sender, options, results, code=0):
                path = "/org/freedesktop/portal/desktop/request/" + sender[1:].replace(".", "_") + "/" + str(options["handle_token"])
                request = Request(path); self.objects.append(request)
                if code is not None:
                    GLib.timeout_add(80, lambda: (request.Response(dbus.UInt32(code), dbus.Dictionary(results, signature="sv")), False)[1])
                return request
            @dbus.service.method("org.freedesktop.portal.GlobalShortcuts", in_signature="a{sv}", out_signature="o", sender_keyword="sender")
            def CreateSession(self, options, sender=None):
                path = "/org/freedesktop/portal/desktop/session/" + sender[1:].replace(".", "_") + "/" + str(options["session_handle_token"])
                self.session = Session(path); self.objects.append(self.session)
                return self.request(sender, options, {"session_handle": dbus.String(path, variant_level=1)})._object_path
            def shortcuts(self):
                return dbus.Array([dbus.Struct(("capture", dbus.Dictionary({"description": "Capture", "trigger_description": self.trigger}, signature="sv")), signature="sa{sv}")], signature="(sa{sv})", variant_level=1)
            @dbus.service.method("org.freedesktop.portal.GlobalShortcuts", in_signature="oa{sv}", out_signature="o", sender_keyword="sender")
            def ListShortcuts(self, session, options, sender=None):
                return self.request(sender, options, {"shortcuts": self.shortcuts() if self.saved else dbus.Array([], signature="(sa{sv})", variant_level=1)})._object_path
            @dbus.service.method("org.freedesktop.portal.GlobalShortcuts", in_signature="oa(sa{sv})sa{sv}", out_signature="o", sender_keyword="sender")
            def BindShortcuts(self, session, shortcuts, parent, options, sender=None):
                values = self.shortcuts()
                self.saved = self.saved or self.bind_status == 0
                return self.request(sender, options, {"shortcuts": values}, self.bind_status)._object_path
            @dbus.service.signal("org.freedesktop.portal.GlobalShortcuts", signature="osta{sv}")
            def Activated(self, session, shortcut, timestamp, options): pass
            @dbus.service.method("org.freedesktop.portal.GlobalShortcuts", in_signature="osa{sv}", out_signature="")
            def ConfigureShortcuts(self, session, parent, options):
                self.trigger = "Ctrl+Alt+U"
                self.ShortcutsChanged(session, self.shortcuts())
            @dbus.service.signal("org.freedesktop.portal.GlobalShortcuts", signature="oa(sa{sv})")
            def ShortcutsChanged(self, session, shortcuts): pass
            @dbus.service.method("org.freedesktop.portal.Screenshot", in_signature="sa{sv}", out_signature="o", sender_keyword="sender")
            def Screenshot(self, parent, options, sender=None):
                if self.screenshot_mode is None:
                    raise dbus.exceptions.DBusException("No screenshot backend", name="org.freedesktop.DBus.Error.UnknownMethod")
                if self.screenshot_mode == "denied":
                    raise dbus.exceptions.DBusException("Denied", name="org.freedesktop.portal.Error.NotAllowed")
                self.pending = self.request(sender, options, {}, None)
                return self.pending._object_path

        portal = Portal()
        resident = spawn()
        settings = wait_for(lambda: find_window("尚未启用系统快捷键"), "portal capability found")
        click(settings, "设置系统快捷键")
        wait_for(lambda: "系统快捷键未启用" in text(settings), "cancel authorization never reports enabled")
        portal.bind_status = 0
        click(settings, "设置系统快捷键")
        wait_for(lambda: "系统快捷键已启用：Ctrl+Alt+Y" in text(settings), "actual portal trigger is displayed")
        click(settings, "更改系统快捷键")
        wait_for(lambda: "系统快捷键已启用：Ctrl+Alt+U" in text(settings), "system shortcut changes are reflected")
        screenshot(settings, "shortcut.png")
        bound_session = portal.session
        close(settings)
        settle(.3)
        success = plan("success", "PORTAL_HOTKEY_CAPTURE")
        portal.Activated(bound_session._object_path, "capture", dbus.UInt64(1), {})
        result = wait_for(lambda: find_window(success["text"]), "portal activation runs screenshot flow")
        before = len(http_calls)
        portal.Activated(bound_session._object_path, "capture", dbus.UInt64(1), {})
        settle(.3)
        require(len(http_calls) == before, "duplicate activation does not capture twice")
        close(result)
        portal.screenshot_mode = "pending"
        portal.Activated(bound_session._object_path, "capture", dbus.UInt64(2), {})
        wait_for(lambda: portal.pending, "portal selection pending")
        portal.Activated(bound_session._object_path, "capture", dbus.UInt64(3), {})
        wait_for(lambda: portal.pending.closed, "toggle calls portal Request.Close")
        require(len(http_calls) == before, "cancelled portal screenshot is not uploaded")
        portal.screenshot_mode = "denied"
        portal.Activated(bound_session._object_path, "capture", dbus.UInt64(4), {})
        result = wait_for(lambda: find_window("截图权限被拒绝"), "permission failure is actionable")
        close(result)
        bound_session.Close()
        command()
        settings = wait_for(lambda: find_window("系统已结束快捷键会话"), "session loss disables shortcut state")
        tray("退出 SnapTranslate")
        wait_for(lambda: resident.poll() is not None, "shortcut app exits")
        require(resident.returncode == 0, "shortcut app exits cleanly")
        restored = spawn()
        settings = wait_for(lambda: find_window("系统快捷键已启用：Ctrl+Alt+U"), "authorized shortcut is restored after restart")
        restored_session = portal.session
        tray("退出 SnapTranslate")
        wait_for(lambda: restored.poll() is not None, "restored app exits")
        wait_for(lambda: restored_session.closed, "exit closes the portal shortcut session")
        for obj in reversed(portal.objects): obj.remove_from_connection()
        portal.remove_from_connection()
        bus.release_name("org.freedesktop.portal.Desktop")
        print("PASS portal authorization/cancel/actual trigger/activation/session loss and screenshot Close/permission denial", flush=True)

        native_flows()

    def native_flows():
        init_clipboard()
        def image_url(response):
            return next(part["image_url"]["url"] for message in response["body"]["messages"] if isinstance(message.get("content"), list) for part in message["content"] if part.get("type") == "image_url")
        # Exercise the actual installed X11 selector against generated pixels.
        # Only this private Xvfb screen is visible to maim.
        native_maim = shutil.which("maim")
        if native_maim:
            from gi.repository import Gtk
            fixture_window = Gtk.Window(title="SyntheticCaptureFixture")
            fixture_window.set_default_size(600, 240)
            fixture_window.set_child(Gtk.Label(label="SnapTranslate native capture fixture\nGenerated test pixels only"))
            fixture_window.present()
            settle(.3)
            fake_command = maim.read_text()
            maim.write_text('#!/bin/sh\nexec ' + shlex.quote(native_maim) + ' "$@"\n')
            try:
                # Register an HTTPS handler only in the temporary XDG directory.
                from gi.repository import Gio
                applications = root / "data/applications"
                applications.mkdir(parents=True, exist_ok=True)
                handler = root / "open-uri.py"
                handler.write_text("import pathlib,sys\npathlib.Path(" + repr(str(root / "opened-uri")) + ").write_text(sys.argv[1])\n")
                desktop = applications / "snap-uri-test.desktop"
                desktop.write_text("[Desktop Entry]\nType=Application\nName=Isolated URI test\nExec=/usr/bin/python3 " + str(handler) + " %u\nMimeType=x-scheme-handler/https;\n")
                info = Gio.DesktopAppInfo.new_from_filename(str(desktop))
                require(info.set_as_default_for_type("x-scheme-handler/https"), "private URL handler is installed")
                rich = "NATIVE_SELECTION_OK\nSecond line\n\n| Name | Value |\n| --- | --- |\n| A | 1 |\n\n[Docs](https://example.com/test)"
                real = plan("success", rich)
                native_app = spawn("-v", "capture")
                settle(.8)
                x_events(title="SyntheticCaptureFixture", focus=False, drag=((30, 70), (230, 150)))
                result = wait_for(lambda: find_window("NATIVE_SELECTION_OK"), "real maim region selection completes", timeout=12)
                import struct
                payload = base64.b64decode(image_url(real).split(",", 1)[1])
                width, height = struct.unpack(">II", payload[16:24])
                require(180 <= width <= 220 and 60 <= height <= 100, "real screenshot has selected region dimensions")
                (root / "native-selection.png").write_bytes(payload)
                wait_for(lambda: "翻译完成" in text(result), "native result is formatted")
                settle(.25)
                content = named(result, "译文内容")
                visible = node_text(content)
                require("NATIVE_SELECTION_OK\nSecond line" in visible and "Name │ Value\nA │ 1" in visible, "final layout preserves lines and table structure")
                point = last_text_point(result, content)
                x_events(title=result.get_name(), click=point, control_click=True)
                wait_for(lambda: (root / "opened-uri").exists(), "Ctrl+click opens link with the private handler")
                require((root / "opened-uri").read_text() == "https://example.com/test", "link destination is preserved")
                screenshot(result, "native-result.png")
                close(result)
                tray("退出 SnapTranslate")
                require(native_app.wait(timeout=5) == 0, "native capture app exits")
                print("PASS actual maim selection via XTest on generated private-desktop pixels", flush=True)
            finally:
                maim.write_text(fake_command)
                fixture_window.destroy()


    def edge_flows():
        credentials = config / "credentials.toml"
        saved_credentials = credentials.read_bytes()
        credentials.unlink()
        before = len(requests)
        missing_key = spawn("capture")
        try:
            wait_for(lambda: find_window("请先输入 API Key"), "missing key opens actionable settings")
            require(len(requests) == before, "missing key never sends an HTTP request")
            tray("退出 SnapTranslate")
            require(missing_key.wait(timeout=5) == 0, "missing-key application exits via tray")
        finally:
            credentials.write_bytes(saved_credentials)
            credentials.chmod(0o600)
        print("PASS first capture without credentials opens settings and sends no request", flush=True)

        escaped = plan("slow", "ESC_CANCEL_REQUEST", ticks=150)
        escape_process = spawn("capture")
        window = wait_for(lambda: find_window(escaped["text"]), "Escape test result")
        x_events(keys=[(0xff1b,)])
        wait_for(lambda: not find_window(escaped["text"]), "Escape closes result")
        wait_for(lambda: escaped.get("disconnected"), "Escape cancels its network request", timeout=4)
        tray("退出 SnapTranslate")
        require(escape_process.wait(timeout=5) == 0, "Escape test process exits")
        print("PASS Escape closes result and cancels its HTTP connection", flush=True)

        quitting = plan("slow", "QUIT_DURING_REQUEST", ticks=150)
        active = spawn("capture")
        wait_for(lambda: find_window(quitting["text"]), "active request before Quit")
        tray("退出 SnapTranslate")
        require(active.wait(timeout=5) == 0, "Quit exits during active translation")
        wait_for(lambda: quitting.get("disconnected"), "Quit disconnects active network request", timeout=4)
        print("PASS tray Quit ends an active translation and closes its connection", flush=True)

        (root / "slow-capture").touch()
        selecting = spawn("capture")
        selector_pid = int(wait_for(lambda: (root / "selector.pid").read_text() if (root / "selector.pid").exists() else None, "delayed selector process starts"))
        tray("退出 SnapTranslate")
        require(selecting.wait(timeout=5) == 0, "Quit exits while selecting a region")
        wait_for(lambda: not Path(f"/proc/{selector_pid}").exists(), "Quit reaps the pending screenshot subprocess", timeout=4)
        (root / "slow-capture").unlink()
        print("PASS Quit cancels and reaps a pending screenshot subprocess", flush=True)

        # A new app now runs in the same isolated desktop without any tray host.
        bus.release_name("org.kde.StatusNotifierWatcher")
        watcher.remove_from_connection()
        no_tray = spawn()
        settings = wait_for(lambda: find_window("当前桌面未显示系统托盘"), "no-tray guidance is visible")
        close(settings)
        wait_for(lambda: not windows(), "no-tray settings closes")
        require(no_tray.poll() is None, "no-tray app remains available after closing settings")
        command()
        settings = wait_for(lambda: find_window("当前桌面未显示系统托盘"), "relaunch restores no-tray settings")
        click(settings, "退出应用")
        require(no_tray.wait(timeout=5) == 0, "settings Quit works without a tray")
        print("PASS no-tray guidance, close/reopen settings, and explicit Quit", flush=True)

    try:
        if native_only:
            native_flows()
        elif parity_only:
            parity_flows()
        elif recovery_only:
            recovery_flows()
        elif reading_only:
            resident = spawn()
            close(wait_for(lambda: find_window("DeepSeek API Key"), "reading test settings"))
            reading_flows(resident)
            tray("退出 SnapTranslate")
            require(resident.wait(timeout=5) == 0, "reading test exits")
        else:
            if not edge_only and not ux_only:
                standard_flows()
            if not edge_only:
                ux_flows()
            if not edge_only and not ux_only:
                recovery_flows()
            if not edge_only and not ux_only:
                parity_flows()
            if not ux_only:
                edge_flows()
        require(len(http_calls) == len(requests), "no unexpected duplicate HTTP requests occurred")
        print(f"PASS all GUI flows ({len(requests)} local HTTP requests)", flush=True)
    except Exception:
        try:
            (root / "accessible-windows.txt").write_text("\n\n".join(text(w) for w in windows()))
            for index, window in enumerate(windows()):
                screenshot(window, f"failure-{index}.png")
        except Exception:
            pass
        raise
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        server.shutdown()
        for logfile in logfiles:
            logfile.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/debug/snaptranslate-linux")
    parser.add_argument("--keep-artifacts", action="store_true")
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--edge-only", action="store_true", help="Run only startup / shutdown / Escape edge cases")
    modes.add_argument("--ux-only", action="store_true", help="Run only settings / clipboard / reading interactions")
    modes.add_argument("--reading-only", action="store_true", help="Run only streaming scroll interactions")
    modes.add_argument("--recovery-only", action="store_true", help="Run only retry / stop / settings recovery flows")
    modes.add_argument("--parity-only", action="store_true", help="Run queue, capture toggle, and global shortcut portal flows")
    modes.add_argument("--native-only", action="store_true", help="Run actual maim region selection on a generated private desktop")
    parser.add_argument("--worker", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        worker(args.worker, args.binary, args.edge_only, args.ux_only, args.reading_only, args.recovery_only, args.parity_only, args.native_only)
        return
    for executable in ["Xvfb", "xvfb-run", "dbus-run-session"]:
        if shutil.which(executable) is None:
            parser.error(f"Missing dependency: {executable}")
    if not args.binary.is_file():
        parser.error(f"Binary missing: {args.binary}; run cargo build first")
    root = Path(tempfile.mkdtemp(prefix="snaptranslate-gui-flow-"))
    try:
        bus_config = root / "bus.conf"
        bus_config.write_text('<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen><policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>')
        env = dict(os.environ)
        for variable in ["DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE", "AT_SPI_BUS_ADDRESS", "WAYLAND_DISPLAY", "SESSION_MANAGER", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"]:
            env.pop(variable, None)
        for name in ["CONFIG", "DATA", "CACHE", "STATE"]:
            env[f"XDG_{name}_HOME"] = str(root / name.lower())
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        env.update(XDG_RUNTIME_DIR=str(runtime), GDK_BACKEND="x11", GTK_A11Y="atspi", GIO_USE_VFS="local", GTK_USE_PORTAL="0", XDG_SESSION_TYPE="x11", GSK_RENDERER="cairo", LC_ALL="C.UTF-8", NO_PROXY="127.0.0.1,localhost")
        command = ["dbus-run-session", "--config-file", str(bus_config), "--", "xvfb-run", "-a", "-s", "-screen 0 1280x900x24", sys.executable, str(Path(__file__).resolve()), "--worker", str(root), "--binary", str(args.binary.resolve())]
        if args.edge_only:
            command.append("--edge-only")
        if args.ux_only:
            command.append("--ux-only")
        if args.reading_only:
            command.append("--reading-only")
        if args.recovery_only:
            command.append("--recovery-only")
        if args.parity_only:
            command.append("--parity-only")
        if args.native_only:
            command.append("--native-only")
        result = subprocess.run(command, env=env, timeout=160)
        if result.returncode:
            for log in sorted(root.glob("app-*.log")):
                print(f"\n{log.name}:\n{log.read_text()[-5000:]}", file=sys.stderr)
            raise SystemExit(result.returncode)
    finally:
        if args.keep_artifacts:
            print(f"Test artifacts: {root}", flush=True)
        else:
            shutil.rmtree(root)


if __name__ == "__main__":
    main()
