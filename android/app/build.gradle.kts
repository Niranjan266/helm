plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

// Release signing: env first, then ~/.helm (kept outside the repository). Without a keystore,
// assembleRelease builds an unsigned APK, so contributors and CI are never blocked; debug builds
// always use the normal debug key.
val helmKeyDir = File(System.getProperty("user.home"), ".helm")
val releaseKeystore = (System.getenv("HELM_ANDROID_KEYSTORE")?.takeIf { it.isNotBlank() }?.let(::File)
    ?: File(helmKeyDir, "android-release.p12")).takeIf { it.isFile }
val releaseKeystorePassword = System.getenv("HELM_ANDROID_KEYSTORE_PASSWORD")?.takeIf { it.isNotBlank() }
    ?: File(helmKeyDir, "android-keystore-password.txt").takeIf { it.isFile }?.readText()?.trim()

// The Ed25519 key that verifies downloaded updates: HELM_UPDATE_PUBKEY if set (empty = updates
// off), else the committed public key shared with the Windows app.
val updatePublicKey = (System.getenv("HELM_UPDATE_PUBKEY")
    ?: rootProject.file("../keys/update-ed25519.pub").takeIf { it.isFile }?.readText()
    ?: "").trim()

android {
    namespace = "com.niranjan.helm"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.niranjan.helm"
        minSdk = 26
        targetSdk = 34
        versionCode = 4
        versionName = "0.3.0"
        buildConfigField("String", "UPDATE_PUBLIC_KEY", "\"$updatePublicKey\"")
    }

    signingConfigs {
        if (releaseKeystore != null && releaseKeystorePassword != null) {
            create("release") {
                storeFile = releaseKeystore
                storePassword = releaseKeystorePassword
                keyAlias = System.getenv("HELM_ANDROID_KEY_ALIAS")?.takeIf { it.isNotBlank() } ?: "helm-release"
                keyPassword = System.getenv("HELM_ANDROID_KEY_PASSWORD")?.takeIf { it.isNotBlank() } ?: releaseKeystorePassword
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

// The opt-in E2E test reads HELM_E2E_URI; make it a task input so changing it re-runs tests,
// and show the test's printed results in the console.
tasks.withType<Test>().configureEach {
    inputs.property("helmE2eUri", providers.environmentVariable("HELM_E2E_URI").orElse(""))
    inputs.property("helmE2eRelay", providers.environmentVariable("HELM_E2E_RELAY").orElse(""))
    testLogging {
        events("passed", "skipped", "failed")
        showStandardStreams = true
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
    debugImplementation(libs.androidx.compose.ui.tooling)

    implementation(libs.kotlinx.coroutines.core)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.coroutines.play.services)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.okhttp)
    // Ed25519 verification of downloaded updates (Android has no built-in Ed25519 before API 33).
    implementation(libs.bouncycastle.bcprov)
    implementation(libs.play.services.code.scanner)
    implementation(libs.androidx.biometric)
    implementation(libs.androidx.fragment)
    // Wear OS companion relay (G7)
    implementation(project(":wearlink"))
    implementation(libs.play.services.wearable)

    testImplementation(libs.junit)
    testImplementation(libs.okhttp.mockwebserver)
    testImplementation(libs.okhttp.tls)
}
