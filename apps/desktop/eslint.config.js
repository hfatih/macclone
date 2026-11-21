import ts from 'typescript-eslint'
import vue from 'eslint-plugin-vue'
export default [
 ...ts.configs.recommended,
 ...vue.configs['flat/essential'],
 {files:['**/*.vue'],languageOptions:{parserOptions:{parser:ts.parser}}},
 {rules:{'vue/multi-word-component-names':'off'}}
]
