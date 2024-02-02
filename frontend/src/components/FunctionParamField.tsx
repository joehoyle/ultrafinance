import Input from "./forms/Input";

interface Props { id: string, param: { name: string, type: string }, value: string }

export default function FunctionParamField(props: Props) {
	return <label className="mt-2 flex flex-col items-start">
		<span className="text-xs text-gray-500 mb-2">{props.param.name}</span>
		<ParamField id={`params[${props.id}]`} param={props.param} value={props.value} />
	</label>
}

function ParamField({ id, param, value }: Props) {
	switch (param.type) {
		case 'string':
			return <Input defaultValue={value} name={id} />
	}

	return <span>Unknown field type</span>;
}
