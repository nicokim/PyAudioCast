"""Tests for device enumeration."""

import sys

import pytest

import pyaudiocast as pyspeaker
from tests.conftest import requires_audio


def test_list_output_devices_returns_list():
    """list_output_devices should return a list."""
    devices = pyspeaker.list_output_devices()
    assert isinstance(devices, list)


def test_device_dict_has_expected_keys():
    """Each device dict should have 'name', 'id', 'index' and 'type' keys."""
    devices = pyspeaker.list_output_devices()
    for i, dev in enumerate(devices):
        assert isinstance(dev["name"], str)
        assert isinstance(dev["id"], str)
        assert dev["index"] == i
        assert dev["type"] in ("alsa", "pipewire", "wasapi", "coreaudio")
    ids = [d["id"] for d in devices]
    assert len(set(ids)) == len(ids)


@requires_audio
@pytest.mark.skipif(sys.platform != "linux", reason="ALSA device ids")
def test_select_device_by_id():
    """Device ids ("pulse", "hw:CARD=...") are unique and selectable, unlike names."""
    ids = [d["id"] for d in pyspeaker.list_output_devices() if d["type"] == "alsa"]
    if "pulse" not in ids:
        pytest.skip("no pulse ALSA device")
    with pyspeaker.AudioPlayer(device="pulse") as player:
        assert player.is_active


@requires_audio
@pytest.mark.skipif(sys.platform != "linux", reason="PipeWire/PulseAudio sinks")
def test_select_sink_by_id():
    """Sink ids are prefixed, so a sink named like an ALSA device stays selectable."""
    sinks = [d for d in pyspeaker.list_output_devices() if d["type"] == "pipewire"]
    if not sinks:
        pytest.skip("no PipeWire/PulseAudio sinks")
    assert all(d["id"] == "pipewire:" + d["name"] for d in sinks)
    with pyspeaker.AudioPlayer(device=sinks[0]["id"]) as player:
        assert player.is_active
