// super() is only valid in derived constructors.
class Base {
  constructor() {
    super();
  }
}

new Base();
