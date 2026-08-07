plugins {
    id("com.android.application")
}

android {
    namespace = "dev.flm.client"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.flm.client"
        minSdk = 24
        targetSdk = 36
        versionCode = 1
        versionName = "0.1-poc"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
