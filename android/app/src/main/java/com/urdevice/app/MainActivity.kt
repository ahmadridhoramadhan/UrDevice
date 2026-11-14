package com.urdevice.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.tooling.preview.Preview
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.urdevice.core.coreVersion

private val Forest = Color(0xFF0F1F10)
private val Lime = Color(0xFFC6F19D)
private val Mist = Color(0xFFE8F5DC)

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            // Calls into the Rust core through the UniFFI bindings.
            val version = remember { coreVersion() }
            UrDeviceTheme { HomeScreen(version) }
        }
    }
}

@Composable
private fun UrDeviceTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(primary = Lime, background = Forest, onBackground = Mist),
        content = content,
    )
}

@Composable
private fun HomeScreen(version: String) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(Forest),
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Image(
            painter = painterResource(R.drawable.ic_urdevice_symbol),
            contentDescription = null,
            modifier = Modifier.size(96.dp),
        )
        Text(text = stringResource(R.string.app_name), color = Lime, fontSize = 28.sp)
        Text(text = stringResource(R.string.core_version, version), color = Mist, fontSize = 16.sp)
    }
}

@Preview
@Composable
private fun HomeScreenPreview() {
    UrDeviceTheme { HomeScreen("0.1.0") }
}
