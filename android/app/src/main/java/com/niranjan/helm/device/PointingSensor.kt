package com.niranjan.helm.device

import android.content.Context
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import com.niranjan.helm.core.TiltPointer

/**
 * Where the phone's top edge points, for the presentation laser: (yaw, pitch) in radians from
 * the game rotation vector (no magnetometer, so no compass jumps; yaw is only used relative to
 * the calibration), falling back to the rotation vector.
 */
class PointingSensor(context: Context, private val onSample: (Double, Double) -> Unit) : SensorEventListener {
    private val sm = context.getSystemService(Context.SENSOR_SERVICE) as SensorManager
    private val sensor: Sensor? =
        sm.getDefaultSensor(Sensor.TYPE_GAME_ROTATION_VECTOR) ?: sm.getDefaultSensor(Sensor.TYPE_ROTATION_VECTOR)

    val available: Boolean get() = sensor != null

    fun start() {
        sensor?.let { sm.registerListener(this, it, SensorManager.SENSOR_DELAY_GAME) }
    }

    fun stop() = sm.unregisterListener(this)

    override fun onSensorChanged(event: SensorEvent) {
        val r = TiltPointer.matrixFromRotationVector(event.values)
        val (yaw, pitch) = TiltPointer.pointing(r)
        onSample(yaw, pitch)
    }

    override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) = Unit
}
