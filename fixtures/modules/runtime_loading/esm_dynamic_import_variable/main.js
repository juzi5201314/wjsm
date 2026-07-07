const part = './dep';
import(part + '.js').then(ns => console.log(ns.value));
