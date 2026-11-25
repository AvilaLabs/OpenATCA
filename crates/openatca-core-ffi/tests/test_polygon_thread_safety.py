"""Synthetic lifetime checks for the provisional ctypes polygon handle."""

from __future__ import annotations

import ctypes
import sys
import threading
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "python"))

from openatca import Polygon, _MAX_POLYGON_VERTICES  # noqa: E402


class _FakeLibrary:
    def __init__(self) -> None:
        self.contains_entered = threading.Event()
        self.release_contains = threading.Event()
        self.freed: list[int] = []
        self.free_called = threading.Event()

    def oa_geo_polygon_new_wgs84(self, _latitudes, _longitudes, _count, output, _error) -> int:
        output._obj.value = 0x1234
        return 0

    def oa_geo_polygon_contains_wgs84(self, _polygon, _lat, _lon, inside, _error) -> int:
        self.contains_entered.set()
        if not self.release_contains.wait(2):
            return 1
        inside._obj.value = 1
        return 0

    def oa_geo_polygon_edge_count(self, _polygon, output, _error) -> int:
        output._obj.value = 3
        return 0

    def oa_geo_polygon_free(self, polygon) -> None:
        self.freed.append(int(polygon.value))
        self.free_called.set()


class _ObservedLock:
    """Expose the close thread's failed nonblocking acquisition without timing sleeps."""

    def __init__(self, lock: threading.Lock) -> None:
        self._lock = lock
        self.close_blocked = threading.Event()

    def __enter__(self) -> "_ObservedLock":
        if threading.current_thread().name == "polygon-close":
            if not self._lock.acquire(blocking=False):
                self.close_blocked.set()
                self._lock.acquire()
        else:
            self._lock.acquire()
        return self

    def __exit__(self, _type, _value, _traceback) -> None:
        self._lock.release()


class PolygonLifetimeTests(unittest.TestCase):
    def test_close_waits_for_inflight_native_call_and_frees_once(self) -> None:
        library = _FakeLibrary()
        polygon = Polygon(library, [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0)])
        observed_lock = _ObservedLock(polygon._lock)
        polygon._lock = observed_lock  # type: ignore[assignment]
        failures: list[BaseException] = []

        def run_contains() -> None:
            try:
                self.assertTrue(polygon.contains(0.25, 0.25))
            except BaseException as error:
                failures.append(error)

        def run_close() -> None:
            try:
                polygon.close()
            except BaseException as error:
                failures.append(error)

        contains_thread = threading.Thread(target=run_contains, name="polygon-contains")
        close_thread = threading.Thread(target=run_close, name="polygon-close")
        contains_thread.start()
        self.assertTrue(library.contains_entered.wait(2))
        close_thread.start()
        self.assertTrue(observed_lock.close_blocked.wait(2))
        self.assertFalse(library.free_called.is_set())

        library.release_contains.set()
        contains_thread.join(2)
        close_thread.join(2)
        self.assertFalse(contains_thread.is_alive())
        self.assertFalse(close_thread.is_alive())
        self.assertEqual(failures, [])
        self.assertEqual(library.freed, [0x1234])

        polygon.close()
        self.assertEqual(library.freed, [0x1234])
        with self.assertRaises(RuntimeError):
            polygon.contains(0.25, 0.25)

    def test_polygon_input_iteration_stops_at_first_excess_vertex(self) -> None:
        reads = 0

        def unbounded_vertices():
            nonlocal reads
            while True:
                reads += 1
                yield (0.0, 0.0)

        with self.assertRaisesRegex(ValueError, "at most 100001 vertices"):
            Polygon(object(), unbounded_vertices())  # type: ignore[arg-type]
        self.assertEqual(reads, _MAX_POLYGON_VERTICES + 1)


if __name__ == "__main__":
    unittest.main()
