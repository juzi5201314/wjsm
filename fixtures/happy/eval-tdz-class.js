var caught = null;
try {
  eval('typeof C; class C {}');
} catch (e) {
  caught = e.constructor.name;
}
print(caught);
