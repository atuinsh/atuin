module.exports = {
  presets: [
    [
      '@babel/preset-env',
      {
        targets: {
          browsers: [
            // Best practice: https://github.com/babel/babel/issues/7789
            '>=1%',
            'not ie 11',
            'not op_mini all'
          ]
        }
      }
    ]
  ]
};