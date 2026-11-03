package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class StreamFullscreenTest {
    @Test
    fun activeOnlyInLandscapeUntilExited() {
        assertTrue(StreamFullscreen.active(landscape = true, exited = false))
        assertFalse(StreamFullscreen.active(landscape = true, exited = true))
        assertFalse(StreamFullscreen.active(landscape = false, exited = false))
    }

    @Test
    fun boxIsCappedByPreset() {
        assertEquals(1920 to 1080, StreamFullscreen.streamBox(2400, 1080, 1920, 1080, 2560, 1440))
        assertEquals(1280 to 720, StreamFullscreen.streamBox(2400, 1080, 1280, 720, 2560, 1440))
    }

    @Test
    fun boxKeepsPcAspectAndIgnoresOrientationOfInput() {
        assertEquals(1728 to 1080, StreamFullscreen.streamBox(1080, 2400, 1920, 1080, 1920, 1200))
        assertEquals(1280 to 720, StreamFullscreen.streamBox(1280, 720, 1920, 1080, 3840, 2160))
    }

    @Test
    fun boxNeverUpscalesPcAndIsEven() {
        assertEquals(1280 to 720, StreamFullscreen.streamBox(2400, 1080, 1920, 1080, 1280, 720))
        assertEquals(1920 to 1078, StreamFullscreen.streamBox(2401, 1079, 1921, 1081))
    }

    @Test
    fun unknownPcUsesTheBox() {
        assertEquals(1920 to 1080, StreamFullscreen.streamBox(2400, 1080, 1920, 1080))
    }
}
