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
        if (args.length != 5) {
            throw new IllegalArgumentException(
                "参数应为：<scenario> <allocations> <retained> <seed> <workload_hash>"
            );
        }
        String scenario = args[0];
        int allocations = Integer.parseInt(args[1]);
        int retained = Integer.parseInt(args[2]);
        long seed = Long.parseLong(args[3]);
        String workloadHash = args[4];
        Object[] roots = new Object[Math.max(1, retained)];
        long started = System.nanoTime();

        switch (scenario) {
            case "churn", "idle-uncommit" -> churn(allocations, retained, roots);
            case "request" -> request(allocations, retained, seed, roots);
            case "chain" -> chain(allocations, retained, roots);
            case "cycle" -> cycle(allocations, retained, roots);
            case "wide" -> wide(allocations, retained, roots);
            case "mutation" -> mutation(allocations, retained, roots);
            case "humongous" -> humongous(allocations, retained, roots);
            case "saturation" -> saturation(allocations, retained, roots);
            default -> throw new IllegalArgumentException("未知 scenario: " + scenario);
        }
        if ("churn".equals(scenario) || "idle-uncommit".equals(scenario)) {
            System.gc();
        }

        long elapsed = System.nanoTime() - started;
        System.out.printf(
            "{\"steady_state_ns\":%d,\"objects\":%d,\"roots\":%d,\"workload_hash\":\"%s\"}%n",
            elapsed,
            allocations,
            retained,
            workloadHash
        );
    }

    private static void churn(int allocations, int retained, Object[] roots) {
        for (int i = 0; i < allocations; i++) {
            retain(i, retained, roots, new Node(i, roots[i % roots.length], null));
        }
    }

    private static void request(int allocations, int retained, long seed, Object[] roots) {
        for (int i = 0; i < allocations; i++) {
            Node header = new Node(i, null, null);
            Object[] body = {i, seed};
            retain(i, retained, roots, new Node(i, roots[i % roots.length], new Object[] {header, body}));
        }
    }

    private static void chain(int allocations, int retained, Object[] roots) {
        Node tail = null;
        for (int i = 0; i < allocations; i++) {
            tail = new Node(i, tail, null);
            retain(i, retained, roots, tail);
        }
    }

    private static void cycle(int allocations, int retained, Object[] roots) {
        Node first = new Node(0, null, null);
        Node tail = first;
        for (int i = 1; i < allocations; i++) {
            Node node = new Node(i, null, null);
            tail.next = node;
            tail = node;
            retain(i, retained, roots, node);
        }
        tail.next = first;
    }

    private static void wide(int allocations, int retained, Object[] roots) {
        for (int i = 0; i < allocations; i++) {
            Object[] payload = {
                new Node(i, null, null),
                new Node(i + 1L, null, null),
                new Node(i + 2L, null, null),
                new Node(i + 3L, null, null),
            };
            retain(i, retained, roots, new Node(i, null, payload));
        }
    }

    private static void mutation(int allocations, int retained, Object[] roots) {
        for (int i = 0; i < allocations; i++) {
            Node node = new Node(i, roots[i % roots.length], null);
            node.next = new Node(i + 1L, null, null);
            retain(i, retained, roots, node);
        }
    }

    private static void humongous(int allocations, int retained, Object[] roots) {
        for (int i = 0; i < allocations; i++) {
            retain(i, retained, roots, new Node(i, null, new Object[64]));
        }
    }

    private static void saturation(int allocations, int retained, Object[] roots) {
        for (int i = 0; i < allocations; i++) {
            Node left = new Node(i, null, null);
            Node right = new Node(i + 1L, null, null);
            retain(i, retained, roots, new Node(i, left, right));
        }
    }

    private static void retain(int index, int retained, Object[] roots, Node node) {
        if (index < retained) {
            roots[index % roots.length] = node;
        }
    }
}
