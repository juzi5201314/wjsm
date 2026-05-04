var obj = {
  value: 10,
  getValue: function() {
    var inner = () => this.value;
    return inner();
  }
};
console.log(obj.getValue());
