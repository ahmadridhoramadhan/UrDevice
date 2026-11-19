// T10.2 probe: ask a headless Chrome for the camera and report what it sees.
// Usage: node t10_2_chrome_cam.mjs  (Chrome started separately with --remote-debugging-port=9333 --use-fake-ui-for-media-stream)
const list = await (await fetch("http://127.0.0.1:9333/json/new?http://127.0.0.1:8765/", { method: "PUT" })).json();
await new Promise(r => setTimeout(r, 1500));
const ws = new WebSocket(list.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
const expr = `(async()=>{
  const s = await navigator.mediaDevices.getUserMedia({video:true});
  const t = s.getVideoTracks()[0], st = t.getSettings();
  const v = document.createElement('video'); v.srcObject = s; v.muted = true; await v.play();
  await new Promise(r => setTimeout(r, 1500));
  const c = document.createElement('canvas'); c.width = v.videoWidth; c.height = v.videoHeight;
  const x = c.getContext('2d'); x.drawImage(v, 0, 0);
  const d = (await navigator.mediaDevices.enumerateDevices()).filter(d => d.kind == 'videoinput').map(d => d.label);
  return JSON.stringify({label: t.label, w: st.width, h: st.height, fps: st.frameRate, devices: d, px: Array.from(x.getImageData(50,300,1,1).data)});
})()`;
ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate", params: { expression: expr, awaitPromise: true, userGesture: true } }));
ws.onmessage = (m) => { const j = JSON.parse(m.data); if (j.id === 1) { console.log(JSON.stringify(j.result)); process.exit(0); } };
setTimeout(() => { console.log("timeout"); process.exit(1); }, 20000);
