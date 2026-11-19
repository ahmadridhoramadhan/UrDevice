package com.urdevice.spike;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.media.projection.MediaProjectionManager;
import android.os.Bundle;

/** T16.1 spike: ask for MediaProjection consent, then hand the result to CaptureService. */
public class MainActivity extends Activity {
    @Override
    protected void onCreate(Bundle b) {
        super.onCreate(b);
        requestPermissions(new String[] {"android.permission.RECORD_AUDIO", "android.permission.POST_NOTIFICATIONS"}, 1);
    }

    @Override
    public void onRequestPermissionsResult(int code, String[] p, int[] r) {
        MediaProjectionManager m = (MediaProjectionManager) getSystemService(Context.MEDIA_PROJECTION_SERVICE);
        startActivityForResult(m.createScreenCaptureIntent(), 2);
    }

    @Override
    protected void onActivityResult(int req, int res, Intent data) {
        android.util.Log.i("SpikeAPC", "consent result=" + res);
        if (res == RESULT_OK) {
            Intent i = new Intent(this, CaptureService.class).putExtra("code", res).putExtra("data", data);
            startForegroundService(i);
        }
        finish();
    }
}
