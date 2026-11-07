package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class DataSaverTest {
    @Test
    fun autoFollowsTheNetworkAndTheSetting() {
        val auto = SaverOverride.AUTO
        assertNull(DataSaver.reason(auto, onMobileDataSetting = true, onMobileData = false, viaRelay = false))
        assertEquals(SaverReason.MOBILE_DATA, DataSaver.reason(auto, true, onMobileData = true, viaRelay = false))
        assertNull(DataSaver.reason(auto, onMobileDataSetting = false, onMobileData = true, viaRelay = false))
        // The relay always saves data, setting or not.
        assertEquals(SaverReason.RELAY, DataSaver.reason(auto, false, false, viaRelay = true))
        assertEquals(SaverReason.RELAY, DataSaver.reason(auto, true, true, viaRelay = true))
    }

    @Test
    fun sessionOverrideWins() {
        assertNull(DataSaver.reason(SaverOverride.OFF, true, true, true))
        assertEquals(SaverReason.MANUAL, DataSaver.reason(SaverOverride.ON, false, false, false))
        assertEquals(SaverReason.MOBILE_DATA, DataSaver.reason(SaverOverride.ON, false, true, false))
        assertEquals(SaverReason.RELAY, DataSaver.reason(SaverOverride.ON, false, false, true))
        assertEquals(SaverOverride.OFF, DataSaver.toggled(active = true))
        assertEquals(SaverOverride.ON, DataSaver.toggled(active = false))
    }

    @Test
    fun saverUsesTheLowPresetAndNormalUsesSettings() {
        val low = DataSaver.choose(SaverReason.MOBILE_DATA, null, "P1080_60", 16_000)
        assertEquals(StreamChoice(RelayStreamPolicy.PRESET, RelayStreamPolicy.DEFAULT_KBPS, loweredForRelay = true), low)
        assertEquals(StreamChoice(StreamPreset.P1080_60, 16_000), DataSaver.choose(null, null, "P1080_60", 16_000))
        // Picks made while saving count only while saving.
        val picked = DataSaver.choose(SaverReason.MANUAL, null, "P1080_60", 0, pick = StreamPreset.P1080_30, pickKbps = 3_000)
        assertEquals(StreamChoice(StreamPreset.P1080_30, 3_000), picked)
        assertEquals(StreamChoice(StreamPreset.P1080_60, 16_000), DataSaver.choose(null, null, "P1080_60", 0, StreamPreset.P720_30, 1_000))
        assertTrue(DataSaver.notice(SaverReason.MOBILE_DATA).contains("mobile data"))
        assertEquals(RelayStreamPolicy.NOTICE, DataSaver.notice(SaverReason.RELAY))
    }

    @Test
    fun qualityFromLatency() {
        assertNull(LinkQuality.of(null))
        assertNull(LinkQuality.of(0))
        assertEquals(LinkQuality.GOOD, LinkQuality.of(20))
        assertEquals(LinkQuality.GOOD, LinkQuality.of(LinkQuality.GOOD_MS))
        assertEquals(LinkQuality.OK, LinkQuality.of(150))
        assertEquals(LinkQuality.WEAK, LinkQuality.of(400))
        assertEquals("Good 42 ms", LinkQuality.statusText(42))
        assertNull(LinkQuality.statusText(null))
    }

    @Test
    fun droppedFramesLowerTheRating() {
        assertEquals(LinkQuality.GOOD, LinkQuality.of(20, dropped = 1, fps = 30))
        assertEquals(LinkQuality.OK, LinkQuality.of(20, dropped = 5, fps = 30))
        assertEquals(LinkQuality.WEAK, LinkQuality.of(150, dropped = 5, fps = 30))
        assertEquals(LinkQuality.WEAK, LinkQuality.of(20, dropped = 15, fps = 15))
        // No RTT yet: rated on frames alone.
        assertEquals(LinkQuality.GOOD, LinkQuality.of(0, dropped = 0, fps = 30))
        assertFalse(LinkQuality.of(0, dropped = 20, fps = 10) == LinkQuality.GOOD)
        assertEquals("30 fps · 8.2 Mbps", LinkQuality.streamText(30, 8_200))
    }
}
