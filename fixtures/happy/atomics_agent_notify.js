var sab = new SharedArrayBuffer(4);
var ta = new Int32Array(sab);
$262.agent.start(`
  $262.agent.receiveBroadcast(function(sab) {
    var ta = new Int32Array(sab);
    Atomics.store(ta, 0, 1);
    Atomics.notify(ta, 0, 1);
    $262.agent.report('done');
  });
`);
$262.agent.broadcast(sab);
Atomics.wait(ta, 0, 0, 5000);
console.log($262.agent.getReport());
console.log(Atomics.load(ta, 0));