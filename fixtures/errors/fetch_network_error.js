(async () => {
    try {
        await fetch("bad-scheme://example.com");
        console.log("no error");
    } catch (e) {
        console.log("error name:", e.name);
        console.log("error message includes fetch:", e.message.includes("fetch"));
    }
})();