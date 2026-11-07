package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class WidgetPowerTest {
    @Test
    fun offersLockSleepShutdownInOrder() {
        assertEquals(listOf("lock", "sleep", "shutdown"), WidgetPower.ACTIONS)
        assertTrue(WidgetPower.ACTIONS.all { it in PowerSchedule.ACTIONS })
        assertTrue(WidgetPower.isSupported("sleep"))
        assertFalse(WidgetPower.isSupported("restart"))
        assertFalse(WidgetPower.isSupported(null))
    }

    @Test
    fun onlyShutdownAsksFirst() {
        assertTrue(WidgetPower.needsConfirm("shutdown"))
        assertFalse(WidgetPower.needsConfirm("lock"))
        assertFalse(WidgetPower.needsConfirm("sleep"))
    }

    @Test
    fun labelsAndConfirmTitle() {
        assertEquals("Lock PC", WidgetPower.label("lock"))
        assertEquals("Shut down", WidgetPower.label("shutdown"))
        assertEquals("Shut down Desk?", WidgetPower.confirmTitle("Desk"))
        assertEquals("Shut down the PC?", WidgetPower.confirmTitle(" "))
        assertEquals("Shut down the PC?", WidgetPower.confirmTitle(null))
    }

    @Test
    fun doneTextImmediateAndScheduled() {
        assertEquals("Locked the PC.", WidgetPower.doneText("lock"))
        assertEquals("Putting the PC to sleep.", WidgetPower.doneText("sleep", null))
        assertEquals("Shutting down the PC.", WidgetPower.doneText("shutdown", 0))
        assertEquals("Shut down in 30 s.", WidgetPower.doneText("shutdown", 29_500))
        assertEquals("Sleep in 2 min.", WidgetPower.doneText("sleep", 61_000))
    }

    @Test
    fun headerPrefersLiveNameAndOnlyClaimsOnlineWhenConnected() {
        assertEquals(WidgetPower.Header("Desk", "Online", true), WidgetPower.header("Desk", "Laptop", true))
        assertEquals(WidgetPower.Header("Laptop", "Not connected", false), WidgetPower.header(null, "Laptop", false))
        assertEquals(WidgetPower.Header("Laptop", "Not connected", false), WidgetPower.header("", "Laptop", false))
        assertEquals("Helm", WidgetPower.header(null, null, false).title)
        assertFalse(WidgetPower.header(null, null, false).online)
    }
}
