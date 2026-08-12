const values = [0, 1, 2, 3];
const removed = values.splice(1, 2, "a", "b", "c");
console.log(removed.join(","), values.join(","), values.length);

const sparse = [1, , 3, 4];
const holes = sparse.splice(1, 2);
console.log(0 in holes, 1 in holes, holes.length, sparse.join(","), 1 in sparse);

const tail = [1, 2, 3];
console.log(tail.splice(-1).join(","), tail.join(","));

const inserted = [1, 4];
console.log(inserted.splice(1, 0, 2, 3).length, inserted.join(","));
