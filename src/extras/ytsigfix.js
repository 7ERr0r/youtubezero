var Kw = {
    Ty: function (a) {
        a.reverse()
    },
    UD: function (a, b) {
        a.splice(0, b)
    },
    CU: function (a, b) {
        var c = a[0];
        a[0] = a[b % a.length];
        a[b % a.length] = c
    }
};

encrypt = function (a) {
    a = a.split("");
    Kw.CU(a, 6);
    Kw.CU(a, 21);
    Kw.Ty(a, 76);
    Kw.UD(a, 2);
    Kw.Ty(a, 30);
    Kw.UD(a, 2);
    Kw.CU(a, 69);
    Kw.Ty(a, 49);
    Kw.UD(a, 2);
    return a.join("")
}
if (typeof require !== 'undefined' && require.main === module) {
    const myArgs = process.argv.slice(2);
    const result = encrypt(myArgs[0])
    console.log("youtube" + "zero_result:" + result)
}
