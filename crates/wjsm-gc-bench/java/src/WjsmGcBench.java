public final class WjsmGcBench {
    private static final class Node {
        private final long id;
        private Object next;
        private Object payload;

        Node(long id, Object next, Object payload) {
            this.id = id;
            this.next = next;
            this.payload = payload;
        }
    }

    private WjsmGcBench() {}

    public static void main(String[] args) {
        if (args.length != 4) {
            throw new IllegalArgumentException("参数应为：<scenario> <allocations> <retained> <seed>");
        }
        String scenario = args[0];
        int allocations = Integer.parseInt(args[1]);
        int retained = Integer.parseInt(args[2]);
        long seed = Long.parseLong(args[3]);
        Object[] roots = new Object[Math.max(1, retained)];
        long started = System.nanoTime();

        for (int i = 0; i < allocations; i++) {
            Node node = switch (scenario) {
                case "chain" -> new Node(i, i == 0 ? null : roots[i % roots.length], null);
                case "cycle" -> new Node(i, roots[i % roots.length], null);
                case "wide" -> new Node(i, null, new Object[] {i, seed, i + 1, i + 2});
                case "mutation" -> {
                    Node value = new Node(i, roots[i % roots.length], null);
                    value.next = new Node(i + 1L, null, null);
                    yield value;
                }
                case "humongous" -> new Node(i, null, new byte[4096]);
                case "request", "saturation", "churn", "idle-uncommit" ->
                    new Node(i, roots[i % roots.length], new long[] {i, seed});
                default -> throw new IllegalArgumentException("未知 scenario: " + scenario);
            };
            if (i < retained) {
                roots[i % roots.length] = node;
            }
        }
        if ("idle-uncommit".equals(scenario)) {
            System.gc();
        }

        long elapsed = System.nanoTime() - started;
        System.out.printf("{\"steady_state_ns\":%d,\"objects\":%d,\"roots\":%d}%n", elapsed, allocations, retained);
    }
}
