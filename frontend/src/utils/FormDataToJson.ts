// @ts-nocheck

function update(data, keys, value) {
	if (keys.length === 0) {
		// Leaf node
		return value;
	}

	let key = keys.shift();
	if (!key) {
		data = data || [];
		if (Array.isArray(data)) {
			key = data.length;
		}
	}

	// Try converting key to a numeric value
	let index = +key;
	if (!isNaN(index)) {
		// We have a numeric index, make data a numeric array
		// This will not work if this is a associative array
		// with numeric keys
		data = data || [];
		key = index;
	}

	// If none of the above matched, we have an associative array
	data = data || {};

	let val = update(data[key], keys, value);
	data[key] = val;

	return data;
}

export default function FormDataToJson<T>(formData: FormData): T {
	return Array.from(formData.entries()).reduce((data, [field, value]) => {
		let [_, prefix, keys] = field.match(/^([^\[]+)((?:\[[^\]]*\])*)/);

		if (keys) {
			keys = Array.from(keys.matchAll(/\[([^\]]*)\]/g), (m) => m[1]);
			value = update(data[prefix], keys, value);
		}
		data[prefix] = value;
		return data;
	}, {});
}
