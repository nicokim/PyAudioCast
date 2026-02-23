"""Tests for device enumeration."""

import pyaudiocast as pyspeaker


def test_list_output_devices_returns_list():
    """list_output_devices should return a list."""
    devices = pyspeaker.list_output_devices()
    assert isinstance(devices, list)


def test_device_dict_has_expected_keys():
    """Each device dict should have 'name' and 'index' keys."""
    devices = pyspeaker.list_output_devices()
    for dev in devices:
        assert "name" in dev
        assert "index" in dev
        assert isinstance(dev["name"], str)
        assert isinstance(dev["index"], int)
