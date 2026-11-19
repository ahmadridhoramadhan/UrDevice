package com.urdevice.spike;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioPlaybackCaptureConfiguration;
import android.media.AudioRecord;
import android.media.projection.MediaProjection;
import android.media.projection.MediaProjectionManager;
import android.os.IBinder;

/** T16.1 spike: record playback audio of other apps for 12 s and log the peak level every 500 ms. */
public class CaptureService extends Service {
    private static final String TAG = "SpikeAPC";

    @Override
    public IBinder onBind(Intent i) { return null; }

    @Override
    public int onStartCommand(Intent in, int flags, int id) {
        NotificationChannel ch = new NotificationChannel("c", "spike", NotificationManager.IMPORTANCE_LOW);
        getSystemService(NotificationManager.class).createNotificationChannel(ch);
        Notification n = new Notification.Builder(this, "c").setContentTitle("APC spike running")
                .setSmallIcon(android.R.drawable.ic_media_play).build();
        startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION);
        MediaProjectionManager m = getSystemService(MediaProjectionManager.class);
        final MediaProjection mp = m.getMediaProjection(in.getIntExtra("code", 0), (Intent) in.getParcelableExtra("data"));
        mp.registerCallback(new MediaProjection.Callback() {
            @Override public void onStop() { android.util.Log.i(TAG, "projection stopped"); }
        }, null);
        new Thread(() -> run(mp)).start();
        return START_NOT_STICKY;
    }

    private void run(MediaProjection mp) {
        try {
            AudioPlaybackCaptureConfiguration cfg = new AudioPlaybackCaptureConfiguration.Builder(mp)
                    .addMatchingUsage(AudioAttributes.USAGE_MEDIA)
                    .addMatchingUsage(AudioAttributes.USAGE_GAME)
                    .addMatchingUsage(AudioAttributes.USAGE_UNKNOWN)
                    .build();
            AudioFormat fmt = new AudioFormat.Builder().setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .setSampleRate(48000).setChannelMask(AudioFormat.CHANNEL_IN_STEREO).build();
            AudioRecord ar = new AudioRecord.Builder().setAudioFormat(fmt).setAudioPlaybackCaptureConfig(cfg)
                    .setBufferSizeInBytes(48000 * 4).build();
            android.util.Log.i(TAG, "AudioRecord state=" + ar.getState() + " (1 = initialized)");
            ar.startRecording();
            short[] buf = new short[4800 * 2];
            long end = System.currentTimeMillis() + 12000, last = System.currentTimeMillis();
            int peak = 0;
            long frames = 0;
            while (System.currentTimeMillis() < end) {
                int r = ar.read(buf, 0, buf.length);
                if (r < 0) { android.util.Log.i(TAG, "read error " + r); break; }
                frames += r / 2;
                for (int i = 0; i < r; i++) peak = Math.max(peak, Math.abs(buf[i]));
                if (System.currentTimeMillis() - last >= 500) {
                    android.util.Log.i(TAG, "frames=" + frames + " peak=" + peak);
                    peak = 0;
                    last = System.currentTimeMillis();
                }
            }
            ar.stop();
            ar.release();
        } catch (Throwable t) {
            android.util.Log.i(TAG, "FAILED " + t);
        }
        mp.stop();
        stopSelf();
    }
}
