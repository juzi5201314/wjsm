let x; try { x = eval("throw 'err'") } catch(e) { x = e } console.log(x)
